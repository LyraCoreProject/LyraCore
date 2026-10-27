use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::dump::TableRows;
use crate::world_import::{census_plan, WorldImportArgs};
use crate::world_source::{SourceKind, WorldSource};

#[derive(Default)]
struct TableCount {
    rows: usize,
    keys: BTreeSet<String>,
    per_entry: BTreeMap<String, usize>,
    per_map: BTreeMap<u32, usize>,
    skill_offers: usize,
}
type Records = BTreeMap<String, TableCount>;

fn records(statements: &[String]) -> Result<Records> {
    let mut output = Records::new();
    for statement in statements {
        let Some(insert) = statement.strip_prefix("INSERT INTO ") else {
            continue;
        };
        let (table, rest) = insert.split_once(" (").context("planned INSERT columns")?;
        let (columns, values) = rest
            .split_once(") VALUES ")
            .context("planned INSERT values")?;
        let columns = columns.split(',').map(str::to_string).collect::<Vec<_>>();
        let mut quoted = false;
        let mut field = String::new();
        let mut row = Vec::new();
        for c in values.chars() {
            match c {
                '\'' => {
                    quoted = !quoted;
                    field.push(c);
                }
                '(' if !quoted && row.is_empty() && field.is_empty() => {}
                ',' if !quoted && (!row.is_empty() || !field.is_empty()) => {
                    row.push(std::mem::take(&mut field))
                }
                ')' if !quoted => {
                    row.push(std::mem::take(&mut field));
                    anyhow::ensure!(row.len() == columns.len(), "{table}: planned row width");
                    let fields = columns
                        .iter()
                        .cloned()
                        .zip(row.drain(..))
                        .collect::<BTreeMap<_, _>>();
                    let count = output.entry(table.to_string()).or_default();
                    count.rows += 1;
                    if table == "game_trainer_spell"
                        && fields
                            .get("learn_skill_line")
                            .is_some_and(|line| line != "0")
                    {
                        count.skill_offers += 1;
                    }
                    for key in keys(&[fields], key_columns(table)?)? {
                        *count.per_entry.entry(key.clone()).or_default() += 1;
                        count.keys.insert(key);
                    }
                }
                ',' if !quoted => {}
                _ => field.push(c),
            }
        }
        anyhow::ensure!(
            !quoted && row.is_empty() && field.is_empty(),
            "{table}: incomplete planned INSERT"
        );
    }
    Ok(output)
}

fn key_columns(table: &str) -> Result<&'static [&'static str]> {
    Ok(match table {
        "game_gameobject_template"
        | "game_gameobject_trap"
        | "game_meeting_stone"
        | "game_transport" => &["entry"],
        "game_go_model" => &["entry"],
        "game_gameobject" => &["template_entry"],
        "game_item_template" => &["entry"],
        "game_trainer_spell" => &["trainer_entry"],
        "game_creature_loot" => &["creature_entry"],
        "game_pickpocket_loot" => &["creature_entry"],
        "game_skinning_loot" => &["skin_loot_id"],
        "game_gameobject_loot" => &["loot_id"],
        "game_fishing_loot" => &["zone_id"],
        "game_npc_vendor" => &["creature_entry"],
        "game_item_property_weight" => &["pool_id"],
        "game_item_random_property" => &["property_id"],
        "game_item_enchantment" => &["enchant_id"],
        _ => bail!("no census key for {table}"),
    })
}

fn keys(rows: &[BTreeMap<String, String>], columns: &[&str]) -> Result<BTreeSet<String>> {
    rows.iter()
        .map(|row| {
            columns
                .iter()
                .map(|column| {
                    row.get(*column)
                        .cloned()
                        .with_context(|| format!("planned row lacks {column}"))
                })
                .collect::<Result<Vec<_>>>()
                .map(|key| key.join(":"))
        })
        .collect()
}

fn ids(values: impl Iterator<Item = String>) -> String {
    let values = values.collect::<Vec<_>>();
    if values.is_empty() {
        "none".into()
    } else {
        values.join(", ")
    }
}

pub(super) fn run(
    classic: &Path,
    vmangos: &Path,
    commit: &str,
    args: &WorldImportArgs,
) -> Result<String> {
    let family = args.family.as_deref().context("census family")?;
    let classic = WorldSource::open(classic, SourceKind::ClassicDb)?;
    let vmangos = WorldSource::open(vmangos, SourceKind::Vmangos)?;
    let before_plan = census_plan(&classic, args)?;
    let after_plan = census_plan(&vmangos, args)?;
    let mut before = records(&before_plan.stmts)?;
    let mut after = records(&after_plan.stmts)?;
    if family == "gameobjects" {
        for (records, plan) in [(&mut before, &before_plan), (&mut after, &after_plan)] {
            let count = records.entry("game_gameobject".into()).or_default();
            for row in plan.go_batches.iter().flat_map(|batch| batch.split(';')) {
                let mut fields = row.split(',');
                let entry = fields
                    .nth(1)
                    .context("planned GameObject entry")?
                    .to_string();
                let map = fields.next().context("planned GameObject map")?.parse()?;
                *count.per_map.entry(map).or_default() += 1;
                *count.per_entry.entry(entry.clone()).or_default() += 1;
                count.keys.insert(entry);
                count.rows += 1;
            }
            anyhow::ensure!(
                count.rows == plan.go_row_count,
                "GameObject census differs from reducer batches"
            );
        }
    }
    if family == "gameobjects" {
        let client = Path::new(args.dbc.as_deref().context("model census client")?);
        for (records, source) in [(&mut before, &classic), (&mut after, &vmangos)] {
            let models = crate::go_model::packed_rows(client, source)?;
            let count = records.entry("game_go_model".into()).or_default();
            for row in &models {
                let entry = row
                    .split(',')
                    .next()
                    .context("planned model entry")?
                    .to_string();
                count.keys.insert(entry.clone());
                count.per_entry.insert(entry, 1);
            }
            count.rows = models.len();
        }
    }
    let tables = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let profile = args
        .world_profile
        .map(|profile| profile.name())
        .unwrap_or("custom");
    let mut output = format!("# {family} census\n\nGenerated by `lyracore-importer --source vmangos --dump <sqlite> --census <classic.sql> --family {family} --dbc <client/Data> --world-profile {profile}`.\n\nvmangos commit: `{commit}`. Classic SHA-256: `{}`. SQLite SHA-256: `{}`.\n\nCounts and keys come from production plans for patch 10 and build 5875, without applying them to a Realm.\n\n| Module table | Classic | vmangos |\n| --- | ---: | ---: |\n", classic.source_hash()?, vmangos.source_hash()?);
    for table in &tables {
        writeln!(
            output,
            "| {table} | {} | {} |",
            before.get(table).map_or(0, |count| count.rows),
            after.get(table).map_or(0, |count| count.rows)
        )?;
    }
    for table in &tables {
        let columns = key_columns(table)?;
        let empty = BTreeSet::new();
        let before_keys = before.get(table).map_or(&empty, |count| &count.keys);
        let after_keys = after.get(table).map_or(&empty, |count| &count.keys);
        writeln!(
            output,
            "\n## {table}\n\nEntry key: `{}`. Counts above include all rows per entry.\n\nAdded ({}): {}.\n\nRemoved ({}): {}.",
            columns.join(":"),
            after_keys.difference(before_keys).count(),
            ids(after_keys.difference(before_keys).cloned()),
            before_keys.difference(after_keys).count(),
            ids(before_keys.difference(after_keys).cloned())
        )?;
    }
    explain(&mut output, family, &vmangos)?;
    floors(&mut output, family, &before, &after)?;
    if let Some(manifest) = after_plan.world_source_manifest {
        let mut manifest: serde_json::Value = serde_json::from_str(&manifest)?;
        if let Some(findings) = manifest["findings"].as_array_mut() {
            for finding in findings {
                if let Some(object) = finding.as_object_mut() {
                    object.remove("source_keys");
                    object.remove("source_ids");
                }
            }
        }
        writeln!(output, "\n## Compatibility Manifest\n\nProduction findings, with source keys omitted here. The import prints the full findings. The approved-value Source Profile gate is due in S6c.\n\n```json\n{}\n```", serde_json::to_string_pretty(&manifest)?)?;
    }
    Ok(output)
}

fn explain(output: &mut String, family: &str, vmangos: &WorldSource) -> Result<()> {
    writeln!(output, "\n## Differences\n")?;
    if family == "items" {
        for table in [
            "item_template",
            "creature_template",
            "item_enchantment_template",
            "creature_loot_template",
            "reference_loot_template",
            "forbidden_items",
        ] {
            writeln!(
                output,
                "- `{table}`: patch and static-condition selection keeps {} of {} source rows.",
                vmangos.rows(table)?.len(),
                vmangos.row_count(table)?
            )?;
        }
        writeln!(output, "- Item template differences are selected catalogue membership; the importer keeps all selected item templates. Items 25, 50, 51 and 52 exist in vmangos and remain diagnostic roots, not selection filters.\n- Creature loot differences include source loot membership, reference expansion and forbidden-item removal. Creature entry keys omit generated loot group ids, which differ between the sources. Creature loot already belongs to the items Import Family.\n- Vendor differences come from direct offers plus shared offers joined through `vendor_id`, after forbidden-item and static-condition removal. The importer keeps one offer per creature and item.\n- Property Pool differences come from the selected enchantment members. vmangos stores four decimal places, so its weights use a common scale of 10,000; classic-db keeps its scale of 100.\n- Random Properties and enchantment effects use the same client tables in both sources.\n- The unused restores_power flag is false for vmangos. Item on-use spells retain food and drink behavior; classic-db keeps its existing output.\n- Full-world selects all creature templates. Scoped profiles select spawn choices and forced entries; EventAI dependency closure remains due in S6.")?;
    }
    if family == "loot" {
        for table in [
            "pickpocketing_loot_template",
            "skinning_loot_template",
            "gameobject_loot_template",
            "fishing_loot_template",
            "reference_loot_template",
            "item_loot_template",
            "disenchant_loot_template",
            "mail_loot_template",
        ] {
            writeln!(
                output,
                "- `{table}`: patch and static-condition selection keeps {} of {} source rows.",
                vmangos.rows(table)?.len(),
                vmangos.row_count(table)?
            )?;
        }
        writeln!(output, "- Pickpocket differences follow selected creature-to-loot links and source drops. Skinning imports distinct loot ids shared by those creatures.\n- Chest loot follows every selected chest template except the eight authored gather nodes. Fishing imports every selected zone. Both retain source membership after patch selection and reference expansion.\n- vmangos zero-chance group members share the probability left by explicit members. Classic-db keeps its existing omission of direct zero-chance rows for byte parity. Forbidden items are removed before reference weights are expanded, so forbidden children do not take a share of the remaining reference weight. Direct quest-only drops retain the sign as a separate flag where the Module table supports it.\n- Item-container and disenchant tables have no World Source import path in the current Module; they are counted in unsupported findings. Mail loot belongs to quests and is integrated in S3f.\n- Reference group-zero entries roll independently, group selectors select only the named group, and maxcount repeats each reference. Largest-remainder allocation keeps grouped shares within the reference chance. The manifest retains variable counts, runtime conditions and quest-only fishing or skinning drops. S6 supplies EventAI dependency closure for scoped creature selections.")?;
    }
    if family == "trainers" {
        for table in ["npc_trainer", "npc_trainer_template"] {
            writeln!(
                output,
                "- `{table}`: build selection keeps {} of {} source rows.",
                vmangos.rows(table)?.len(),
                vmangos.row_count(table)?
            )?;
        }
        writeln!(output, "- Offer differences follow the selected creature catalogue and direct/shared trainer lists. Shared offers join through `trainer_id` and take precedence for duplicate spell IDs. Only templates with the trainer flag receive offers.\n- vmangos imports the source teach-spell wrappers and its constant skill-step effects. It does not synthesize the classic-db profession list. The manifest's profession audit compares offered skill targets with `PROFESSION_LEARN`. Each skill step raises the cap by 75 ranks.\n- A zero source required level falls back to the selected wrapper's spell level. Source skill/rank requirements and multiple learn targets are reported as unsupported because the current Module cannot carry them in a trainer offer.\n- Scoped profiles use selected creature entries; EventAI dependency closure remains due in S6.")?;
    }
    if family == "gameobjects" {
        for table in ["gameobject_template", "gameobject", "transports"] {
            writeln!(
                output,
                "- `{table}`: patch/build selection keeps {} of {} source rows.",
                vmangos.rows(table)?.len(),
                vmangos.row_count(table)?
            )?;
        }
        writeln!(output, "- Template differences are selected catalogue membership, with real type-25 fishing holes omitted because the Module uses 25 for authored gather nodes. Trap and meeting-stone rows follow those selected templates.\n- Spawn differences follow source placements, profile coordinates, patch ranges, disabled flags and omitted fishing-hole templates. Source ready state maps to Module closed state. Active chests also map to closed so their loot remains usable; active doors map to open. Alternative-active stays 2. The Module converts its closed/open values back to client READY/ACTIVE states when sending packets.\n- The eight gather entries still use the authored skill, item and threshold rules. The source's additional loot is unsupported.\n- Transport counts include the production transport catalogue builder for both sources. Classic-db runs it in the client catalogue pass; vmangos runs it with gameobjects. Model counts use the separate `--go-models` builder with the same client meshes; empty meshes are omitted. Routes use unchanged client TaxiPath and TaxiPathNode tables; the first path node supplies the initial map.\n- Spawn entry keys identify templates with placed spawns, not source spawn GUIDs. No Realm was modified.")?;
    }
    Ok(())
}

fn floors(output: &mut String, family: &str, before: &Records, after: &Records) -> Result<()> {
    writeln!(output, "\n## Consumed floors\n\nMinimums come from `importer/scripts/import-manifest.sh`. These are planned rows. Spawn coverage also needs the creatures family, due in S4.\n\n| Floor | Minimum | Classic | vmangos |\n| --- | ---: | ---: | ---: |")?;
    if family == "items" {
        writeln!(
            output,
            "| VENDORS | {} | {} | {} |",
            super::floor("VENDORS")?,
            before.get("game_npc_vendor").map_or(0, |x| x.rows),
            after.get("game_npc_vendor").map_or(0, |x| x.rows)
        )?;
        writeln!(output, "\n`VENDOR_COVERAGE` minimum {} and `SENTINEL_HILL_VENDOR` minimum {} require spawned entities. Their Realm counts cannot be proved by an items-only plan; recheck with the creatures family in S4.", super::floor("VENDOR_COVERAGE")?, super::floor("SENTINEL_HILL_VENDOR")?)?;
    }
    if family == "loot" {
        for (name, table) in [
            ("PICKPOCKET_LOOT", "game_pickpocket_loot"),
            ("SKINNING_LOOT", "game_skinning_loot"),
            ("GAMEOBJECT_CHEST_LOOT", "game_gameobject_loot"),
            ("FISHING_LOOT", "game_fishing_loot"),
        ] {
            writeln!(
                output,
                "| {name} | {} | {} | {} |",
                super::floor(name)?,
                before.get(table).map_or(0, |x| x.rows),
                after.get(table).map_or(0, |x| x.rows)
            )?;
        }
        writeln!(output, "\n`CREATURES_WITH_PICKPOCKET`, `CREATURES_WITH_SKIN` and `DEADMINES_NAMED_LOOT` depend on the creatures family and its spawned entities. Their minimum is one, one and two respectively; verify them after S4.")?;
    }
    if family == "trainers" {
        for (name, skills) in [
            ("TRAINER_OFFERINGS_CLASS", false),
            ("TRAINER_OFFERINGS_PROFESSION", true),
        ] {
            let count = |records: &Records| {
                records.get("game_trainer_spell").map_or(0, |count| {
                    if skills {
                        count.skill_offers
                    } else {
                        count.rows - count.skill_offers
                    }
                })
            };
            writeln!(
                output,
                "| {name} | {} | {} | {} |",
                super::floor(name)?,
                count(before),
                count(after)
            )?;
        }
        writeln!(output, "\n`CLASS_TRAINERS` minimum {} and `TRAINER_COVERAGE` minimum {} need spawned creatures and remain for S4. The profession floor counts every skill-grant offer, including riding.", super::floor("CLASS_TRAINERS")?, super::floor("TRAINER_COVERAGE")?)?;
    }
    if family == "gameobjects" {
        let gather = |records: &Records| {
            records.get("game_gameobject").map_or(0, |count| {
                count
                    .per_entry
                    .iter()
                    .filter(|(entry, _)| {
                        entry
                            .parse::<u64>()
                            .ok()
                            .is_some_and(|id| crate::world_import::gather_node(id).is_some())
                    })
                    .map(|(_, count)| count)
                    .sum::<usize>()
            })
        };
        writeln!(
            output,
            "| GATHER_NODE_GO | {} | {} | {} |",
            super::floor("GATHER_NODE_GO")?,
            before.get("game_gameobject").map_or(0, |count| count.rows),
            after.get("game_gameobject").map_or(0, |count| count.rows)
        )?;
        for (name, map) in [
            ("DEADMINES_GAMEOBJECTS", 36),
            ("CORRIDOR_GAMEOBJECTS map 0", 0),
            ("CORRIDOR_GAMEOBJECTS map 1", 1),
        ] {
            let floor = name.split(' ').next().unwrap();
            let count = |records: &Records| {
                records
                    .get("game_gameobject")
                    .and_then(|count| count.per_map.get(&map))
                    .copied()
                    .unwrap_or(0)
            };
            writeln!(
                output,
                "| {name} | {} | {} | {} |",
                super::floor(floor)?,
                count(before),
                count(after)
            )?;
        }
        writeln!(output, "\nThe script's `GATHER_NODE_GO` query counts every GameObject spawn. Actual authored gather-node spawns are {} classic and {} vmangos. The corridor floors apply only to the profile's included continents. These are planned counts; a Realm import remains a later integration check.", gather(before), gather(after))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planned_rows_keep_quoted_delimiters_inside_text() {
        let parsed = records(&[
            "INSERT INTO game_item_template (entry,name) VALUES (1,'A),(''B'),(2,'雪')".into(),
        ])
        .unwrap();
        assert_eq!(parsed["game_item_template"].rows, 2);
        assert_eq!(
            parsed["game_item_template"].keys,
            BTreeSet::from(["1".into(), "2".into()])
        );
    }
}
