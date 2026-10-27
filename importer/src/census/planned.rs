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
    npc_flags: BTreeMap<String, u32>,
    quest_bands: [usize; 3],
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
                    if table == "game_creature_template" {
                        count
                            .npc_flags
                            .insert(fields["entry"].clone(), fields["npc_flags"].parse()?);
                    }
                    if table == "game_trainer_spell"
                        && fields
                            .get("learn_skill_line")
                            .is_some_and(|line| line != "0")
                    {
                        count.skill_offers += 1;
                    }
                    if table == "game_quest_template" {
                        let level = fields["quest_level"].parse::<u32>()?;
                        count.quest_bands[0] += usize::from((6..=10).contains(&level));
                        count.quest_bands[1] += usize::from((10..=20).contains(&level));
                        count.quest_bands[2] +=
                            usize::from(fields["next_quest_id"].parse::<u32>()? != 0);
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
        "game_creature_ai_broadcast_text" | "game_creature_ai_summon" => &["id"],
        "game_world_state_name" => &["variable_id"],
        "game_faction" => &["faction_id"],
        "game_faction_template" | "game_area" | "game_area_trigger" | "game_taxi_node" => &["id"],
        "game_skill_ability" => &["spell_id", "skill_line"],
        "game_spell_learn" => &["parent_spell", "learn_spell"],
        "game_spell"
        | "game_spell_chain"
        | "game_spell_proc_event"
        | "game_spell_effect"
        | "game_spell_reagent"
        | "game_creature_ai_spell_metadata" => &["spell_id"],
        "game_creature_template"
        | "game_creature_level_stats"
        | "game_creature_display"
        | "game_creature_spawn" => &["entry"],
        "game_creature_waypoint" => &["creature_guid"],
        "game_creature_population" => &["population_id"],
        "game_creature_population_member" => &["source_guid"],
        "game_creature_cast" | "game_creature_spell" => &["creature_entry"],
        "game_gameobject_template"
        | "game_gameobject_trap"
        | "game_meeting_stone"
        | "game_transport" => &["entry"],
        "game_gossip_menu" | "game_gossip_option" => &["entry"],
        "game_npc_text" | "game_npc_text_slot" => &["text_id"],
        "game_gossip_menu_profile" | "game_gossip_menu_profile_option" => &["menu_id"],
        "game_gossip_condition" => &["condition_id"],
        "game_go_model" => &["entry"],
        "game_gameobject" => &["template_entry"],
        "game_item_template" => &["entry"],
        "game_trainer_spell" => &["trainer_entry"],
        "game_quest_template" => &["entry"],
        "game_quest_text"
        | "game_quest_objective"
        | "game_quest_cast_objective"
        | "game_quest_reward_item"
        | "game_quest_reward_spell"
        | "game_quest_reward_choice"
        | "game_quest_reward_mail"
        | "game_quest_event_requirement" => &["quest_entry"],
        "game_creature_quest" => &["creature_entry", "quest_entry"],
        "game_gameobject_quest" => &["go_entry", "quest_entry"],
        "game_mail_loot" => &["mail_template_id"],
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
    let classic = if family == "creature-ai" {
        WorldSource::ClassicDb(crate::dump::ParsedDump::parse_script_census(
            crate::dump::read_dump_text(classic.to_str().context("classic census path")?)?,
        )?)
    } else {
        WorldSource::open(classic, SourceKind::ClassicDb)?
    };
    let vmangos = WorldSource::open(vmangos, SourceKind::Vmangos)?;
    let mut classic_args = args.clone();
    classic_args.source = SourceKind::ClassicDb;
    classic_args.eventai_profile = crate::eventai::SOURCE_PROFILE_NAME.into();
    let before_plan = census_plan(&classic, &classic_args)?;
    let after_plan = census_plan(&vmangos, args)?;
    let mut before = records(&before_plan.stmts)?;
    let mut after = records(&after_plan.stmts)?;
    if family == "creatures" {
        for (records, plan) in [(&mut before, &before_plan), (&mut after, &after_plan)] {
            let count = records.entry("game_creature_spawn".into()).or_default();
            for row in plan.spawn_batches.iter().flat_map(|batch| batch.split(';')) {
                let fields: Vec<_> = row.split(',').collect();
                let entry = fields[1].to_string();
                let map = fields[2].parse()?;
                *count.per_map.entry(map).or_default() += 1;
                *count.per_entry.entry(entry.clone()).or_default() += 1;
                count.keys.insert(entry);
                count.rows += 1;
            }
            anyhow::ensure!(
                count.rows == plan.spawn_row_count,
                "creature census differs from reducer batches"
            );
        }
    }
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
    if family == "creatures" {
        let old = &before["game_creature_waypoint"];
        let new = &after["game_creature_waypoint"];
        writeln!(output, "\nWaypoint points {} → {}; {} creature GUIDs removed, {} added. Source path membership and point counts differ; vmangos uses spawn paths before template paths and emits only movement type 2. Classic path selection is unchanged.",
            old.rows, new.rows, old.keys.difference(&new.keys).count(), new.keys.difference(&old.keys).count())?;
    }
    if family == "spellmeta" {
        for (table, reason) in [
            ("game_spell_chain", "The sources supply different explicit rank families and skill-line rank links; vmangos adds the five Shadow Vulnerability ranks."),
            ("game_spell_proc_event", "The selected source proc overlays differ in membership; these counts do not count the spell catalogue's own proc fields."),
            ("game_spell_learn", "The four removed links teach TBC riding spells 33388 or 33391 from class mounts 5784, 13819, 23161 and 23214; they do not belong to build 5875."),
        ] {
            let old = &before[table];
            let new = &after[table];
            writeln!(output, "\n`{table}` {} → {}; {} removed, {} added. {reason}", old.rows, new.rows, old.keys.difference(&new.keys).count(), new.keys.difference(&old.keys).count())?;
        }
    }
    if family == "creature-ai" {
        super::eventai::append(&mut output, &classic, &vmangos, &before_plan, &after_plan)?;
    }
    explain(&mut output, family, &vmangos)?;
    floors(&mut output, family, &before, &after)?;
    if family == "creatures" {
        creature_coverage(&mut output, &classic, &vmangos, args, &before, &after)?;
    }
    if let Some(manifest) = after_plan.world_source_manifest {
        let mut manifest: serde_json::Value = serde_json::from_str(&manifest)?;
        super::eventai::summarize_scripts(&mut manifest);
        for group in ["findings", "limitations"] {
            if let Some(findings) = manifest[group].as_array_mut() {
                for finding in findings {
                    if let Some(object) = finding.as_object_mut() {
                        object.remove("source_keys");
                        object.remove("source_ids");
                    }
                }
            }
        }
        writeln!(output, "\n## Compatibility Manifest\n\nProduction findings, with source keys omitted here. The import prints the full findings. The vmangos Source Profile checks columns and approved values before planning; counts do not gate the import.\n\n```json\n{}\n```", serde_json::to_string_pretty(&manifest)?)?;
    }
    Ok(output)
}

fn creature_coverage(
    output: &mut String,
    classic: &WorldSource,
    vmangos: &WorldSource,
    args: &WorldImportArgs,
    before: &Records,
    after: &Records,
) -> Result<()> {
    writeln!(output, "\n## Spawn candidate coverage\n\nCandidates include every population choice, including inactive event and unsupported nested-pool positions. These are upper bounds on live coverage. A complete Realm import is still required to verify live floors.\n\n| Service | Floor | Classic candidates with service / flagged | vmangos candidates with service / flagged |\n| --- | ---: | ---: | ---: |")?;
    let populations = [
        crate::creature_population::collect(classic, &args.scope)?,
        crate::creature_population::collect(vmangos, &args.scope)?,
    ];
    for (family, table, flag, floor) in [
        ("items", "game_npc_vendor", 4, "VENDOR_COVERAGE"),
        ("trainers", "game_trainer_spell", 16, "TRAINER_COVERAGE"),
        ("quests", "game_creature_quest", 2, "QUESTGIVER_COVERAGE"),
    ] {
        let mut values = Vec::new();
        for (index, (source, records)) in [(classic, before), (vmangos, after)]
            .into_iter()
            .enumerate()
        {
            let mut service_args = args.clone();
            service_args.family = Some(family.into());
            let service = records_of_service(source, &service_args, table)?;
            let flagged: BTreeSet<_> = records["game_creature_template"]
                .npc_flags
                .iter()
                .filter(|(entry, bits)| {
                    **bits & flag != 0
                        && populations[index]
                            .referenced_entries
                            .contains(&entry.parse::<u64>().unwrap())
                })
                .map(|(entry, _)| entry.clone())
                .collect();
            values.push(format!(
                "{} / {}",
                flagged.intersection(&service).count(),
                flagged.len()
            ));
        }
        writeln!(
            output,
            "| {family} | {} | {} | {} |",
            super::floor(floor)?,
            values[0],
            values[1]
        )?;
    }
    for (label, records) in [("Classic", before), ("vmangos", after)] {
        let spawns = &records["game_creature_spawn"];
        writeln!(output, "\n{label} spawn positions by map: {:?}. The Deadmines floor is {} positions; the continent floor is {}. Population limits and events reduce live counts.", spawns.per_map, super::floor("DEADMINES_CREATURE_SPAWNS")?, super::floor("CONTINENT_SPAWNS")?)?;
    }
    Ok(())
}

fn records_of_service(
    source: &WorldSource,
    args: &WorldImportArgs,
    table: &str,
) -> Result<BTreeSet<String>> {
    let plan = census_plan(source, args)?;
    Ok(records(&plan.stmts)?
        .get(table)
        .map_or_else(BTreeSet::new, |count| {
            count
                .keys
                .iter()
                .map(|key| key.split(':').next().unwrap().to_string())
                .collect()
        }))
}

fn explain(output: &mut String, family: &str, vmangos: &WorldSource) -> Result<()> {
    writeln!(output, "\n## Differences\n")?;
    if family == "creature-ai" {
        writeln!(output, "- The sources have different event populations and numbering. vmangos supplies entry rules for EventAI templates; classic-db also supplies spawn-GUID rules. Profile scope and accepted template dependencies determine subjects.\n- A vmangos event carries up to three ordered command lists. Unsupported commands, targets, conditions or dependencies drop the complete rule. The source and emitted counts therefore measure different things.\n- Only generic scripts reachable from accepted EventAI roots become relays. The other six delayed-script families have no current classic importer dispatcher; quest-end mail already uses the vmangos quest columns.\n- Broadcast membership follows accepted talk instructions. Summon-location rows and world-state names differ because the vmangos commands needing those catalogues remain unsupported. Event-credit requirements for vmangos quests belong to the quests family; classic-db derives them from accepted AI instructions.")?;
    }
    if family == "dbc" {
        writeln!(output, "- All six eligible tables use source rows selected for build 5875, with area_template unversioned. Every consumed column has a source equivalent. The field-level changes are in dbc-diff.md.\n- AreaTrigger adds 17 IDs and changes geometry on 34 matched IDs. TaxiNodes changes 13 matched IDs. The other catalogues retain their client IDs; source skill abilities use source row IDs instead of generated ordinals.\n- Trigger cooldowns, conditions and scripts remain unsupported. Separate client tables still supply taxi routes, path geometry and display validation. Map stays entirely on Map.dbc.")?;
    }
    if family == "spellmeta" {
        writeln!(output, "- Spell chains, proc overlays and dependent learns use build ranges. Missing explicit rank families are derived from selected skill_line_ability rows. Proc effect selection uses vmangos spell_template. Existing plate and mail dependent learns remain when absent from source rows. Inactive learn visibility and unrepresentable proc masks have manifest findings.")?;
    }
    if family == "spell" {
        writeln!(output, "- The complete spell catalogue uses the selected vmangos spell_template rows in both profiles. The same derived rows supply the Base Snapshot. SpellCastTimes, SpellDuration, SpellRadius and SpellRange remain separate client tables.\n- The detailed field comparison is in dbc-diff.md. Existing effect translation and synthetic fishing markers remain. Source scripts and the high 32 bits of spell modifier masks remain unsupported.")?;
    }
    if family == "creatures" || family == "casts" {
        for table in [
            "creature_template",
            "creature",
            "creature_spells",
            "creature_movement",
            "creature_movement_template",
            "pool_template",
            "game_event_creature",
            "creature_groups",
        ] {
            writeln!(
                output,
                "- `{table}`: {} source rows, {} selected rows.",
                vmangos.row_count(table)?,
                vmangos.rows(table)?.len()
            )?;
        }
        writeln!(output, "- Patch ranges, disabled placements and profile boundaries change spawn membership. The `id` through `id5` columns supply equal entry choices. Classic spawn groups have no source counterpart; vmangos retains pools and signed event membership.
- Creature health, melee damage and armor come from selected class-level stats and template multipliers. Rank rates default to one in vmangos. Imported level rows prevent the Module from adding its classic rank multipliers. Client display scales resolve zero template scales; source weights select a display on each spawn.
- The cast family joins creature spell-list links to eight slots. Its first spell supplies the cast row, with later spells using the existing rotation model. Slot target, chance, timing and script detail remain unsupported.
- `audit-creature-ids.py` reports source GUID counts in each reserved band. The broad curated interval uses a different key space from spawn GUIDs.
- Source behavior reference: `Creature.cpp`, `CreatureDefines.h` and `World.cpp` at vmangos `464179081673cfd240f7ffe7f0daf96bca0b5a70`. EventAI template dependencies now participate in profile scope.")?;
    }
    if family == "gossip" {
        for table in [
            "broadcast_text",
            "npc_text",
            "gossip_menu",
            "gossip_menu_option",
            "conditions",
        ] {
            writeln!(
                output,
                "- `{table}`: {} selected source rows.",
                vmangos.rows(table)?.len()
            )?;
        }
        writeln!(output, "- Creature greetings and options follow selected creature menu links. Scoped entries use selected spawn choices and forced entries; EventAI template dependencies also participate in profile scope.
- vmangos imports every menu as an addressable profile. Classic-db keeps its four relay profiles. Source script menu targets use this catalogue; untranslated script dispatch remains unsupported.
- Text and weighted-slot differences follow source broadcast references and unconditional greetings. The last unconditional greeting wins. Dynamic greeting conditions remain unsupported, so a conditional-only menu uses the source default 0xffffff. Gendered strings, literal NULL text and Unicode are preserved.
- Questgiver placeholder options are omitted, as in classic-db. Creature options apply source NPC flag masks and menu-zero fallback. Profile options cannot store those masks. Gossip submenus use the addressable profiles and evaluate their option conditions. Talent reset requires a class trainer of the Character's class. Unsupported actions and payment/input gates cannot be clicked. POIs, confirmations and source script delivery remain unsupported.
- Condition rows cover the full source table. The importer maps supported source meanings into LyraCore kinds, resolves patch conditions at patch 10 and marks other source kinds unavailable. Unknown conditions stay unavailable through NOT and OR. Stored codes stay compatible with the classic-db baseline.")?;
    }
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
        writeln!(output, "- Item template differences are selected catalogue membership; the importer keeps all selected item templates. Items 25, 50, 51 and 52 exist in vmangos and remain diagnostic roots, not selection filters.\n- Creature loot differences include source loot membership, reference expansion and forbidden-item removal. Creature entry keys omit generated loot group ids, which differ between the sources. Creature loot already belongs to the items Import Family.\n- Vendor differences come from direct offers plus shared offers joined through `vendor_id`, after forbidden-item and static-condition removal. The importer keeps one offer per creature and item.\n- Property Pool differences come from the selected enchantment members. vmangos stores four decimal places, so its weights use a common scale of 10,000; classic-db keeps its scale of 100.\n- Random Properties and enchantment effects use the same client tables in both sources.\n- The unused restores_power flag is false for vmangos. Item on-use spells retain food and drink behavior; classic-db keeps its existing output.\n- Full-world selects all creature templates. Scoped profiles select spawn choices and forced entries; EventAI template dependencies now participate in profile scope.")?;
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
    if family == "quests" {
        for table in [
            "quest_template",
            "creature_questrelation",
            "creature_involvedrelation",
            "gameobject_questrelation",
            "gameobject_involvedrelation",
            "areatrigger_involvedrelation",
            "mail_loot_template",
        ] {
            writeln!(
                output,
                "- `{table}`: selection keeps {} of {} source rows.",
                vmangos.rows(table)?.len(),
                vmangos.row_count(table)?
            )?;
        }
        writeln!(output, "- Quest templates and text follow selected giver relations in the profile's shared quest catalogue. Full profiles use the full catalogue; other canonical profiles share the starter and instance catalogue. Relations emitted into a Realm use local enabled spawn choices. EventAI template dependencies participate in profile scope.\n- Objective differences follow selected quest targets and items available from the catalogue's creature loot, chests and vendors. Forbidden items are removed. Unavailable collect targets and non-goober GameObject targets remain omitted and are counted in the manifest. Cast and area-trigger objectives follow their selected relations.\n- Fixed and choice reward differences follow selected quest rows. Learn rewards use a single-effect learn wrapper from selected source spells. Other reward casts are unsupported.\n- Mail differences follow quest-template mail IDs, delays and money. vmangos reads no classic-db quest-end script table. Used mail loot must be one unconditional item with 100% chance and no reference.\n- Event requirements come from source exploration/event flags, except quests whose area-trigger objective already supplies that completion gate. Unsupported source script/event delivery remains in the compatibility audit; the manifest reports those requirements. Classic-db derives its event requirements from the current EventAI plan.\n- vmangos supplies explicit `RewXP`. The Module cannot distinguish zero XP from its fallback calculation; the manifest counts that limit, along with unrepresented prerequisites, scripts, flags and extra reputation fields.")?;
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
        writeln!(output, "- Offer differences follow the selected creature catalogue and direct/shared trainer lists. Shared offers join through `trainer_id` and take precedence for duplicate spell IDs. Only templates with the trainer flag receive offers.\n- vmangos imports the source teach-spell wrappers and its constant skill-step effects. It does not synthesize the classic-db profession list. The manifest's profession audit compares offered skill targets with `PROFESSION_LEARN`. Each skill step raises the cap by 75 ranks.\n- A zero source required level falls back to the selected wrapper's spell level. Source skill/rank requirements and multiple learn targets are reported as unsupported because the current Module cannot carry them in a trainer offer.\n- Scoped profiles use selected creature entries; EventAI template dependencies now participate in profile scope.")?;
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
    if family == "creature-ai" {
        writeln!(output, "\n## Consumed floors\n\n`importer/scripts/import-manifest.sh` defines no EventAI or relay count floor. Creature, quest and dungeon checks need their other Import Families and a populated Realm.")?;
        return Ok(());
    }
    if family == "dbc" {
        writeln!(output, "\n## Consumed floors\n\n| Floor | Minimum | Classic | vmangos |\n| --- | ---: | ---: | ---: |")?;
        for (floor, table) in [
            ("AREAS", "game_area"),
            ("AREA_TRIGGERS", "game_area_trigger"),
            ("TAXI_NODES", "game_taxi_node"),
        ] {
            writeln!(
                output,
                "| {floor} | {} | {} | {} |",
                super::floor(floor)?,
                before[table].rows,
                after[table].rows
            )?;
        }
        writeln!(output, "\nTaxiPath and TaxiPathNode remain client tables. This slice replaces only TaxiNodes; two reserved fixture nodes remain. Map stays on Map.dbc because map_template lacks internal_name. There are no count floors for faction, faction_template or skill_line_ability.")?;
        return Ok(());
    }
    if family == "spellmeta" {
        writeln!(output,"\n## Consumed floors\n\n| Floor | Minimum | Classic | vmangos |\n| --- | ---: | ---: | ---: |")?;
        for (floor, table) in [
            ("SPELL_CHAIN", "game_spell_chain"),
            ("SPELL_LEARN", "game_spell_learn"),
            ("SPELL_PROC_EVENT", "game_spell_proc_event"),
        ] {
            writeln!(
                output,
                "| {floor} | {} | {} | {} |",
                super::floor(floor)?,
                before[table].rows,
                after[table].rows
            )?;
        }
        return Ok(());
    }
    if family == "spell" {
        writeln!(output, "\n## Consumed floors\n\n| Floor | Minimum | Classic | vmangos |\n| --- | ---: | ---: | ---: |\n| SPELL_TOTAL | {} | {} | {} |",super::floor("SPELL_TOTAL")?,before["game_spell"].rows,after["game_spell"].rows)?;
        return Ok(());
    }
    if family == "gossip" {
        writeln!(output, "\n## Consumed floors\n\nThe import manifest defines no gossip-specific count floor. The creatures census reports creature-service coverage.")?;
        return Ok(());
    }
    writeln!(output, "\n## Consumed floors\n\nMinimums come from `importer/scripts/import-manifest.sh`. These are planned rows. Spawn coverage also needs the creatures family.\n\n| Floor | Minimum | Classic | vmangos |\n| --- | ---: | ---: | ---: |")?;
    if family == "items" {
        writeln!(
            output,
            "| VENDORS | {} | {} | {} |",
            super::floor("VENDORS")?,
            before.get("game_npc_vendor").map_or(0, |x| x.rows),
            after.get("game_npc_vendor").map_or(0, |x| x.rows)
        )?;
        writeln!(output, "\n`VENDOR_COVERAGE` minimum {} and `SENTINEL_HILL_VENDOR` minimum {} require spawned entities. Their Realm counts cannot be proved by an items-only plan; recheck with the creatures family.", super::floor("VENDOR_COVERAGE")?, super::floor("SENTINEL_HILL_VENDOR")?)?;
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
        writeln!(output, "\n`CREATURES_WITH_PICKPOCKET`, `CREATURES_WITH_SKIN` and `DEADMINES_NAMED_LOOT` depend on the creatures family and its spawned entities. Their minimum is one, one and two respectively; verify them on the imported Realm.")?;
    }
    if family == "quests" {
        writeln!(
            output,
            "| QUEST_GIVER_RELATIONS | {} | {} | {} |",
            super::floor("QUEST_GIVER_RELATIONS")?,
            before
                .get("game_creature_quest")
                .map_or(0, |count| count.rows),
            after
                .get("game_creature_quest")
                .map_or(0, |count| count.rows)
        )?;
        for (index, name) in ["QUESTS_L6_10", "QUESTS_L10_20", "QUESTS_CHAINED"]
            .into_iter()
            .enumerate()
        {
            let count = |records: &Records| {
                records
                    .get("game_quest_template")
                    .map_or(0, |count| count.quest_bands[index])
            };
            writeln!(
                output,
                "| {name} | {} | {} | {} |",
                super::floor(name)?,
                count(before),
                count(after)
            )?;
        }
        writeln!(output, "\n`QUESTGIVER_COVERAGE` minimum {} and `CORRIDOR_QUEST_LEVEL_BAND` minimum {} need spawned creatures at the script's locations; verify them on the imported Realm.", super::floor("QUESTGIVER_COVERAGE")?, super::floor("CORRIDOR_QUEST_LEVEL_BAND")?)?;
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
        writeln!(output, "\n`CLASS_TRAINERS` minimum {} and `TRAINER_COVERAGE` minimum {} need spawned creatures. The profession floor counts every skill-grant offer, including riding.", super::floor("CLASS_TRAINERS")?, super::floor("TRAINER_COVERAGE")?)?;
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
