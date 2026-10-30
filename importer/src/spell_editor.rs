use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use wow_dbc::{DbcTable, Indexable};

use crate::dump::TableRows;
use crate::spell::{derive_spell_rows, SpellTables};
use crate::spell_snapshot::TrainerRow;
use crate::world_source::{Cell, Row, TableBuilder, WorldSource};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    create: bool,
    source: BTreeMap<String, String>,
    trainers: Vec<TrainerRow>,
}

struct Source(Vec<Row>);

impl TableRows for Source {
    fn rows(&self, table: &str) -> Result<Cow<'_, [Row]>> {
        ensure!(
            table == "spell_template",
            "only spell_template is available"
        );
        Ok(Cow::Borrowed(&self.0))
    }

    fn source_hash(&self) -> Result<String> {
        bail!("an editor batch has no World Source revision")
    }
}

fn source_rows(rows: &[BTreeMap<String, String>]) -> Result<Source> {
    let mut out = Vec::new();
    for row in rows {
        let mut table = TableBuilder::new("spell_template", row.keys().cloned());
        table.push(
            row.iter()
                .map(|(name, value)| {
                    if matches!(
                        name.as_str(),
                        "name" | "nameSubtext" | "description" | "auraDescription" | "script_name"
                    ) {
                        Cell::Text(value.clone().into())
                    } else {
                        Cell::Number(value.clone().into())
                    }
                })
                .collect(),
        )?;
        out.extend(table.finish());
    }
    Ok(Source(out))
}

fn check_spells(spells: &[crate::spell_dbc::Spell], tables: &SpellTables) -> Result<BTreeSet<u32>> {
    let mut ids = BTreeSet::new();
    for spell in spells {
        ensure!(
            spell.id != 0 && ids.insert(spell.id),
            "zero or duplicate spell id {}",
            spell.id
        );
        ensure!(
            spell.school <= 6,
            "spell {}: school must be 0 through 6",
            spell.id
        );
        ensure!(
            spell
                .effect_base_points
                .iter()
                .all(|v| v.abs_diff(0) < 100_000_000)
                && spell
                    .effect_die_sides
                    .iter()
                    .all(|v| v.abs_diff(0) < 100_000_000),
            "spell {}: effect magnitude is too large",
            spell.id
        );
        ensure!(
            spell.casting_time_index == 0
                || tables
                    .cast_times
                    .get(
                        wow_dbc::vanilla_tables::spell_cast_times::SpellCastTimesKey::new(
                            spell.casting_time_index
                        )
                    )
                    .is_some(),
            "spell {}: missing cast time",
            spell.id
        );
        ensure!(
            spell.range == 0
                || tables
                    .ranges
                    .get(wow_dbc::vanilla_tables::spell_range::SpellRangeKey::new(
                        spell.range
                    ))
                    .is_some(),
            "spell {}: missing range",
            spell.id
        );
        ensure!(
            spell.duration == 0
                || tables
                    .durations
                    .get(
                        wow_dbc::vanilla_tables::spell_duration::SpellDurationKey::new(
                            spell.duration
                        )
                    )
                    .is_some(),
            "spell {}: missing duration",
            spell.id
        );
        for radius in spell.effect_radius {
            ensure!(
                radius == 0
                    || tables
                        .radii
                        .get(wow_dbc::vanilla_tables::spell_radius::SpellRadiusKey::new(
                            radius
                        ))
                        .is_some(),
                "spell {}: missing radius {radius}",
                spell.id
            );
        }
    }
    Ok(ids)
}

fn compile(
    source: Vec<BTreeMap<String, String>>,
    mut tables: SpellTables,
    trainers: &[TrainerRow],
) -> Result<Value> {
    ensure!(
        !source.is_empty() && source.len() <= 64,
        "provide 1 through 64 spell rows"
    );
    let (spells, manifest) = crate::vmangos_spell::read(&source_rows(&source)?)?;
    let ids = check_spells(&spells, &tables)?;
    tables.spells = spells;
    let curated = |row: &&TrainerRow| {
        (crate::spell::CURATED_TRAINER_ID_BASE
            ..crate::spell::CURATED_TRAINER_ID_BASE + crate::spell::CURATED_TRAINER_ID_SPAN)
            .contains(&row.id)
    };
    let offerings = trainers
        .iter()
        .filter(curated)
        .map(|row| (row.trainer_entry, vec![row.spell_id]))
        .collect::<Vec<_>>();
    let (rows, _, _) = derive_spell_rows(
        &tables,
        &ids.iter().copied().collect::<Vec<_>>(),
        &offerings,
    )?;
    let mut effects = BTreeMap::new();
    for effect in rows
        .effects
        .iter()
        .chain(&rows.fishing)
        .filter(|e| ids.contains(&e.spell_id))
    {
        let mut value = serde_json::to_value(effect)?;
        value["id"] = json!(effect.row_id());
        effects.insert(effect.row_id(), value);
    }
    let mut sources = source
        .into_iter()
        .map(|row| Ok((source_entry(&row)?, row)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let mut edits = Vec::new();
    for header in &rows.headers {
        let id = header.spell_id;
        let source = sources
            .remove(&id)
            .with_context(|| format!("spell {id} compiled without a source row"))?;
        let offers = trainers.iter().filter(|r| r.spell_id == id).map(|row| {
            let mut row = row.clone();
            if curated(&&row) {
                let compiled = rows.trainers.iter().find(|r| r.trainer_entry == row.trainer_entry && r.spell_id == id)
                    .with_context(|| format!("spell {id}: include the taught rank when editing its derived trainer offer"))?;
                row.cost = compiled.cost;
                row.required_level = compiled.required_level;
            }
            Ok(row)
        }).collect::<Result<Vec<_>>>()?;
        edits.push(json!({
            "create": false,
            "spell": header,
            "effects": effects.values().filter(|e| e["spell_id"] == id).collect::<Vec<_>>(),
            "reagents": rows.reagents.iter().filter(|r| r.spell_id == id).collect::<Vec<_>>(),
            "metadata": rows.eventai_metadata.iter().filter(|r| r.spell_id == id).collect::<Vec<_>>(),
            "trainers": offers,
            "source": source.into_iter().map(|(name, value)| json!({"name":name,"value":value})).collect::<Vec<_>>()
        }));
    }
    ensure!(
        sources.is_empty(),
        "spells {:?} did not compile",
        sources.keys().collect::<Vec<_>>()
    );
    eprintln!("{}", serde_json::to_string(&manifest)?);
    Ok(Value::Array(edits))
}

fn source_entry(row: &BTreeMap<String, String>) -> Result<u32> {
    row.get("entry")
        .context("spell source has no entry")?
        .parse()
        .context("spell source entry is not a spell id")
}

fn table<T: DbcTable>(dir: &Path) -> Result<T> {
    let path = dir.join(T::FILENAME);
    T::read(&mut Cursor::new(
        std::fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    ))
    .with_context(|| format!("parse {}", path.display()))
}

pub(crate) fn run(input: &str, dbc: &str) -> Result<()> {
    let mut bytes = Vec::new();
    if input == "-" {
        std::io::stdin()
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
    } else {
        std::fs::File::open(input)?
            .take(8 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
    }
    ensure!(bytes.len() <= 8 * 1024 * 1024, "spell input exceeds 8 MiB");
    let requests: Vec<Request> = serde_json::from_slice(&bytes).context("read spell edit JSON")?;
    let dir = Path::new(dbc);
    let tables = SpellTables {
        spells: vec![],
        cast_times: table(dir)?,
        ranges: table(dir)?,
        durations: table(dir)?,
        radii: table(dir)?,
    };
    let trainers = requests
        .iter()
        .flat_map(|r| r.trainers.iter().cloned())
        .collect::<Vec<_>>();
    let mut output = compile(
        requests.iter().map(|r| r.source.clone()).collect(),
        tables,
        &trainers,
    )?;
    let mut requests = requests
        .into_iter()
        .map(|request| Ok((source_entry(&request.source)?, request)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    for edit in output
        .as_array_mut()
        .context("compiled spell array")?
        .iter_mut()
    {
        let id = edit["spell"]["spell_id"]
            .as_u64()
            .context("compiled spell id")? as u32;
        let request = requests
            .remove(&id)
            .with_context(|| format!("spell {id} compiled without a request"))?;
        ensure!(
            request.trainers.iter().all(|r| r.spell_id == id),
            "trainer offer belongs to another spell"
        );
        edit["create"] = json!(request.create);
    }
    println!("{}", serde_json::to_string(&output)?);
    Ok(())
}

pub(crate) fn read_source(path: &str, id: u32) -> Result<()> {
    let source = WorldSource::open(Path::new(path))?;
    let rows = source.rows("spell_template")?;
    let row = rows
        .iter()
        .find(|r| r.number::<u32>("entry").ok() == Some(id))
        .with_context(|| format!("World Source has no spell {id} for build 5875"))?;
    println!("{}", serde_json::to_string(&row.named_values()?)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::TableRows;
    use wow_dbc::vanilla_tables::{
        spell_cast_times::*, spell_duration::*, spell_radius::*, spell_range::*,
    };

    fn fixture() -> (BTreeMap<String, String>, SpellTables) {
        let source = crate::vmangos_spell::fixture();
        let mut row = source.rows("spell_template").unwrap()[0]
            .named_values()
            .unwrap();
        row.insert("script_name".into(), String::new());
        row.insert("effectItemType2".into(), "0".into());
        let tables = SpellTables {
            spells: vec![],
            cast_times: SpellCastTimes {
                rows: vec![SpellCastTimesRow {
                    id: SpellCastTimesKey::new(1),
                    base: 1500,
                    per_level_increase: 0,
                    minimum: 0,
                }],
            },
            ranges: SpellRange {
                rows: vec![SpellRangeRow {
                    id: SpellRangeKey::new(1),
                    range_min: 0.0,
                    range_max: 30.0,
                    flags: 0,
                    display_name: Default::default(),
                    display_name_short: Default::default(),
                }],
            },
            durations: SpellDuration {
                rows: vec![SpellDurationRow {
                    id: SpellDurationKey::new(1),
                    duration: 6000,
                    duration_per_level: 0,
                    max_duration: 12000,
                }],
            },
            radii: SpellRadius { rows: vec![] },
        };
        (row, tables)
    }

    #[test]
    fn edited_rows_compile_without_unrequested_fishing_spells() {
        let (mut row, tables) = fixture();
        row.insert("spellFamilyFlags".into(), u64::MAX.to_string());
        row.insert("effectPointsPerComboPoint1".into(), "2.5".into());
        let output = compile(vec![row], tables, &[]).unwrap();
        let edits = output.as_array().unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0]["spell"]["spell_id"], 10);
        assert_eq!(edits[0]["spell"]["cast_time_ms"], 1500);
        assert_eq!(edits[0]["spell"]["family_flags"], u64::MAX);
        assert!(edits[0]["spell"]["max_duration_ms"].is_number());
        assert_eq!(edits[0]["effects"][0]["points_per_combo"], 2.5);
        assert_eq!(edits[0]["spell"]["range_yd"], 30);
        assert_eq!(edits[0]["effects"][0]["base_points"], 20);
        assert_eq!(edits[0]["reagents"][0]["item_entry"], 123);
        assert_eq!(edits[0]["reagents"][0]["count"], 2);
        assert!(edits[0]["effects"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["spell_id"] == 10));
    }
    #[test]
    fn fishing_replaces_only_the_requested_effect() {
        let (mut row, tables) = fixture();
        row.insert("entry".into(), "7620".into());
        let output = compile(vec![row], tables, &[]).unwrap();
        let effects = output[0]["effects"].as_array().unwrap();
        assert_eq!(effects.iter().filter(|e| e["effect_index"] == 0).count(), 1);
        assert_eq!(effects[0]["kind"], 0x1c);
        assert!(effects.iter().all(|e| e["spell_id"] == 7620));
    }

    #[test]
    fn derived_trainer_prices_change_but_world_source_prices_stay() {
        let (row, tables) = fixture();
        let base = TrainerRow {
            id: 5_200_000,
            trainer_entry: 913,
            spell_id: 10,
            cost: 100,
            required_level: 1,
            learn_skill_line: 0,
            learn_skill_cap: 75,
        };
        let ordinary = TrainerRow {
            id: 100,
            ..base.clone()
        };
        let output = compile(vec![row], tables, &[base, ordinary]).unwrap();
        assert_eq!(output[0]["trainers"][0]["cost"], 350);
        assert_eq!(output[0]["trainers"][0]["required_level"], 7);
        assert_eq!(output[0]["trainers"][1]["cost"], 100);
    }

    #[test]
    fn editor_modes_refuse_apply_and_unrelated_import_modes() {
        for args in [
            vec!["--compile-spell-edits", "-", "--dbc", "dbc", "--apply"],
            vec![
                "--read-spell-source",
                "133",
                "--dump",
                "world.sqlite",
                "--apply",
            ],
            vec!["--compile-spell-edits", "-", "--dbc", "dbc", "--spells"],
        ] {
            assert!(crate::parse_args_from(args).is_err());
        }
    }

    #[test]
    fn malformed_source_rows_and_duplicate_ids_do_not_compile() {
        let (row, tables) = fixture();
        assert!(compile(vec![row.clone(), row], tables, &[])
            .unwrap_err()
            .to_string()
            .contains("duplicate"));
        let (mut row, tables) = fixture();
        row.remove("effect1");
        assert!(compile(vec![row], tables, &[])
            .unwrap_err()
            .to_string()
            .contains("missing column"));
        let (mut row, tables) = fixture();
        row.insert("rangeIndex".into(), "99999".into());
        assert!(compile(vec![row], tables, &[])
            .unwrap_err()
            .to_string()
            .contains("missing range"));
    }
}
