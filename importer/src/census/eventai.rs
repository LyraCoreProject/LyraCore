use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use anyhow::{ensure, Context, Result};

use crate::dump::TableRows;
use crate::world_import::DumpPlan;
use crate::world_source::WorldSource;

#[derive(Default)]
struct Program {
    subjects: BTreeSet<String>,
    rules: usize,
    instructions: usize,
    policies: usize,
    relays: BTreeSet<String>,
    relay_steps: usize,
}

impl Program {
    fn read(plan: &DumpPlan) -> Result<Self> {
        let mut result = Self::default();
        for line in plan
            .eventai_definition_batches
            .iter()
            .flat_map(|b| b.lines())
        {
            let mut fields = line.splitn(3, '@');
            let subject = fields.next().context("AI census subject")?;
            fields.next().context("AI census revision")?;
            let rules = fields.next().context("AI census rules")?;
            ensure!(
                result.subjects.insert(subject.into()),
                "duplicate AI subject"
            );
            for rule in rules.split('~') {
                result.rules += 1;
                result.policies += usize::from(rule.contains(",lists:"));
                let instructions = rule.rsplit(',').next().context("AI census instructions")?;
                ensure!(!instructions.is_empty(), "empty AI census rule");
                result.instructions += instructions.split('+').count();
            }
        }
        for line in plan
            .eventai_relay_definition_batches
            .iter()
            .flat_map(|b| b.lines())
        {
            let fields = line.splitn(5, '@').collect::<Vec<_>>();
            ensure!(fields.len() == 5, "AI census relay definition");
            ensure!(result.relays.insert(fields[0].into()), "duplicate relay id");
            result.relay_steps += fields[4].split('~').filter(|s| !s.is_empty()).count();
        }
        ensure!(
            result.subjects.len() as u64 == plan.eventai_definition_count,
            "AI census definition count"
        );
        ensure!(
            result.relays.len() as u64 == plan.eventai_relay_definition_count,
            "AI census relay count"
        );
        ensure!(
            result.instructions as u64 == plan.eventai_instruction_count,
            "AI census instruction count"
        );
        Ok(result)
    }
}

pub(super) fn append(
    output: &mut String,
    classic: &WorldSource,
    vmangos: &WorldSource,
    before: &DumpPlan,
    after: &DumpPlan,
) -> Result<()> {
    let old = Program::read(before)?;
    let new = Program::read(after)?;
    writeln!(output, "\n## EventAI and relay programs\n\nCounts come from the reducer batches. Rules and instructions are nested in their definitions.\n\n| Measure | Classic | vmangos |\n| --- | ---: | ---: |")?;
    for (label, old, new) in [
        (
            "Definition subjects",
            old.subjects.len(),
            new.subjects.len(),
        ),
        ("Rules", old.rules, new.rules),
        ("EventAI instructions", old.instructions, new.instructions),
        ("Command-list policies", old.policies, new.policies),
        ("Relay definitions", old.relays.len(), new.relays.len()),
        (
            "Relay steps, including control instructions",
            old.relay_steps,
            new.relay_steps,
        ),
    ] {
        writeln!(output, "| {label} | {old} | {new} |")?;
    }
    for (label, old, new) in [
        ("Definition subjects", &old.subjects, &new.subjects),
        ("Relay ids", &old.relays, &new.relays),
    ] {
        let added = new.difference(old).cloned().collect::<Vec<_>>();
        let removed = old.difference(new).cloned().collect::<Vec<_>>();
        writeln!(
            output,
            "\n{label} added ({}): {}.\n\n{label} removed ({}): {}.",
            added.len(),
            listed(&added),
            removed.len(),
            listed(&removed)
        )?;
    }
    writeln!(output, "\n## Source events\n\nSource type and command numbers belong to each source; equal numbers need not mean equal behavior. Classic action slot zero is omitted below because it means no action. vmangos command zero means talk.\n\n| Source | Rules | Subjects | Event types | Event commands or nonempty action slots |\n| --- | ---: | ---: | --- | --- |")?;
    for (label, source, table) in [
        ("Classic", classic, "creature_ai_scripts"),
        ("vmangos", vmangos, "creature_ai_events"),
    ] {
        let rows = source.rows(table)?;
        let mut subjects = BTreeSet::new();
        let mut events = BTreeMap::<u32, usize>::new();
        let mut commands = BTreeMap::<u32, usize>::new();
        for row in rows.iter() {
            subjects.insert(row.number::<i64>("creature_id")?);
            *events.entry(row.number("event_type")?).or_default() += 1;
            if label == "Classic" {
                for slot in 1..=3 {
                    let kind = row.number::<u32>(&format!("action{slot}_type"))?;
                    if kind != 0 {
                        *commands.entry(kind).or_default() += 1;
                    }
                }
            }
        }
        if label == "vmangos" {
            for row in source.rows("creature_ai_scripts")?.iter() {
                *commands.entry(row.number("command")?).or_default() += 1;
            }
        }
        writeln!(
            output,
            "| {label} | {} | {} | {} | {} |",
            rows.len(),
            subjects.len(),
            frequencies(&events),
            frequencies(&commands)
        )?;
    }
    writeln!(output, "\n## Source script tables\n\nThese are selected source rows, before reachability and compatibility filtering. GameObject-use keys are spawn GUIDs. The command maps show `command: count`.\n\n| Source | Table | Rows | Script keys | Commands |\n| --- | --- | ---: | ---: | --- |")?;
    for (classic_table, vmangos_table) in [
        ("dbscripts_on_relay", "generic_scripts"),
        ("dbscripts_on_quest_start", "quest_start_scripts"),
        ("dbscripts_on_quest_end", "quest_end_scripts"),
        ("dbscripts_on_gossip", "gossip_scripts"),
        ("dbscripts_on_event", "event_scripts"),
        (
            "dbscripts_on_creature_movement",
            "creature_movement_scripts",
        ),
        ("dbscripts_on_go_use", "gameobject_scripts"),
    ] {
        for (label, source, table) in [
            ("Classic", classic, classic_table),
            ("vmangos", vmangos, vmangos_table),
        ] {
            let rows = source.rows(table)?;
            let mut keys = BTreeSet::new();
            let mut commands = BTreeMap::<u32, usize>::new();
            for row in rows.iter() {
                keys.insert(row.number::<u32>("id")?);
                *commands.entry(row.number("command")?).or_default() += 1;
            }
            writeln!(
                output,
                "| {label} | {table} | {} | {} | {} |",
                rows.len(),
                keys.len(),
                frequencies(&commands)
            )?;
        }
    }
    Ok(())
}

fn listed(values: &[String]) -> String {
    if values.is_empty() {
        "none".into()
    } else {
        values.join(", ")
    }
}

fn frequencies(values: &BTreeMap<u32, usize>) -> String {
    listed(
        &values
            .iter()
            .map(|(value, count)| format!("{value}: {count}"))
            .collect::<Vec<_>>(),
    )
}

pub(super) fn summarize_scripts(manifest: &mut serde_json::Value) {
    if let Some(tables) = manifest["scripts"].as_object_mut() {
        for table in tables.values_mut() {
            if let Some(table) = table.as_object_mut() {
                if let Some(unsupported) = table.remove("unsupported") {
                    table.insert(
                        "unsupported_scripts".into(),
                        serde_json::json!(unsupported.as_array().map_or(0, Vec::len)),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programs_count_rules_and_relay_steps_without_counting_control_policy_as_an_ai_action() {
        let plan = DumpPlan {
            eventai_definition_batches: vec!["entry:10@1@1,spawn,100,1,once,lists:0:2:,script:0:0:always,phase:0+emote:1:self~2,aggro,100,1,once,all,ordinary,phase:1\nguid:99@2@3,death:always,100,1,once,all,ordinary,phase:2".into()],
            eventai_relay_definition_batches: vec!["20@3@parallel@map-or-instance@0,0,0,source>source,failure-policy:continue~1000,0,1,source>source,phase:1".into()],
            eventai_definition_count: 2,
            eventai_relay_definition_count: 1,
            eventai_instruction_count: 4,
            ..DumpPlan::default()
        };
        let counts = Program::read(&plan).unwrap();
        assert_eq!(counts.rules, 3);
        assert_eq!(counts.policies, 1);
        assert_eq!(counts.relay_steps, 2);
        assert_eq!(
            counts.subjects,
            BTreeSet::from(["entry:10".into(), "guid:99".into()])
        );
        assert_eq!(counts.relays, BTreeSet::from(["20".into()]));
    }
}
