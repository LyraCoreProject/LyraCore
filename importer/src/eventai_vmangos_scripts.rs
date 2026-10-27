use super::{census, joined, Command, Source};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const TABLES: [&str; 7] = [
    "generic_scripts",
    "quest_start_scripts",
    "quest_end_scripts",
    "gossip_scripts",
    "event_scripts",
    "creature_movement_scripts",
    "gameobject_scripts",
];

#[derive(Default)]
pub(super) struct Script {
    steps: Vec<String>,
    dependencies: Vec<u32>,
    pub(super) texts: BTreeSet<u32>,
    pub(super) quests: BTreeSet<u32>,
    scheduled: usize,
}

impl Script {
    pub(super) fn encode(&self, id: u32) -> String {
        let steps = self.steps.join("~");
        let mut hash = blake3::Hasher::new();
        hash.update(b"lyracore-relay-definition-v1");
        hash.update(format!("{id}@parallel@map-or-instance@{steps}").as_bytes());
        let version = u64::from_le_bytes(hash.finalize().as_bytes()[..8].try_into().unwrap());
        format!("{id}@{version}@parallel@map-or-instance@{steps}")
    }
}

pub(super) struct Scripts {
    pub(super) definitions: BTreeMap<u32, Script>,
    findings: BTreeMap<u32, String>,
}

impl Scripts {
    pub(super) fn compile(source: &Source) -> Self {
        let mut scripts = Self {
            definitions: BTreeMap::new(),
            findings: BTreeMap::new(),
        };
        if let Some(rows) = source.delayed.get("generic_scripts") {
            for (id, commands) in rows {
                match compile_script(source, *id, commands) {
                    Ok(script) => {
                        scripts.definitions.insert(*id, script);
                    }
                    Err(reason) => {
                        scripts.findings.insert(*id, reason);
                    }
                }
            }
        }
        scripts
    }

    pub(super) fn closure(&self, roots: &[u32]) -> Result<BTreeSet<u32>, String> {
        let mut included = BTreeSet::new();
        for root in roots
            .iter()
            .copied()
            .filter(|id| *id != 0)
            .collect::<BTreeSet<_>>()
        {
            self.visit(root, &mut Vec::new(), &mut included, &mut [0; 2])?;
        }
        Ok(included)
    }

    fn visit(
        &self,
        id: u32,
        path: &mut Vec<u32>,
        included: &mut BTreeSet<u32>,
        counts: &mut [usize; 2],
    ) -> Result<(), String> {
        if path.contains(&id) || path.len() >= super::super::RELAY_MAX_DEPTH {
            return Err(format!("script:{id}:cycle-or-depth"));
        }
        let script = self.definitions.get(&id).ok_or_else(|| {
            format!(
                "script:{id}:{}",
                self.findings.get(&id).map_or("missing", String::as_str)
            )
        })?;
        counts[0] += script.steps.len();
        counts[1] += script.scheduled;
        if counts[0] > super::super::RELAY_STEP_BUDGET
            || counts[1] > super::super::RELAY_SCHEDULED_WORK_BUDGET
        {
            return Err(format!("script:{id}:work-budget"));
        }
        included.insert(id);
        path.push(id);
        for dependency in &script.dependencies {
            self.visit(*dependency, path, included, counts)?;
        }
        path.pop();
        Ok(())
    }

    pub(super) fn manifest(
        &self,
        tables: &BTreeMap<&str, BTreeMap<u32, Vec<Command>>>,
        used: &BTreeSet<u32>,
    ) -> serde_json::Value {
        let tables = tables.iter().map(|(name, groups)| {
            let findings = groups.keys().filter(|id| *name != "generic_scripts" || !used.contains(id)).map(|id| {
                let reason = if *name == "generic_scripts" {
                    self.closure(&[*id]).err().unwrap_or_else(|| "no-translated-event-starts-script".into())
                } else { "no-classic-import-dispatcher".into() };
                serde_json::json!({"id":id,"reason":reason})
            }).collect::<Vec<_>>();
            ((*name).to_owned(), serde_json::json!({
                "key": if *name == "gameobject_scripts" { "gameobject-guid" } else { "id" },
                "rows":groups.values().map(Vec::len).sum::<usize>(), "subjects":groups.len(),
                "commands":census(groups.values().flatten().map(|c| c.kind)),
                "targets":census(groups.values().flatten().map(|c| c.target)),
                "flags":census(groups.values().flatten().map(|c| c.flags)),
                "unsupported":findings
            }))
        }).collect::<BTreeMap<_, _>>();
        serde_json::json!(tables)
    }
}

pub(super) fn choices(command: &Command) -> Result<Vec<u32>, String> {
    let mut choices = Vec::new();
    for (id, chance) in command.longs.into_iter().zip(command.ints) {
        if id == 0 {
            continue;
        }
        let chance = usize::try_from(chance).map_err(|_| "script-choice-weight")?;
        if super::super::fixture_id(id) || chance > 100 || choices.len() + chance > 100 {
            return Err("script-choice-weight-or-fixture-id".into());
        }
        choices.extend(std::iter::repeat_n(id, chance));
    }
    choices.resize(100, 0);
    Ok(choices)
}

fn compile_script(source: &Source, id: u32, commands: &[Command]) -> Result<Script, String> {
    if id == 0 || super::super::fixture_id(id) {
        return Err("script-id".into());
    }
    let mut script = Script::default();
    let mut policy = None;
    for (order, command) in commands.iter().enumerate() {
        if command.flags & !(4 | 8) != 0 {
            return Err("command-flags-or-swap".into());
        }
        if command.target != 0 || command.target_params != [0, 0] {
            return Err(format!("target:{}", command.target));
        }
        if source
            .predicates
            .get(&command.condition)
            .and_then(Option::as_deref)
            != Some("always")
        {
            return Err(format!("condition:{}", command.condition));
        }
        let offset = command.delay.checked_mul(1000).ok_or("delay-overflow")?;
        let aborts = command.flags & 8 != 0;
        let header = |step_order| format!("{offset},{},{step_order}", command.priority);
        if policy != Some(aborts) {
            script.steps.push(format!(
                "{},source>source,failure-policy:{}",
                header(order * 2),
                if aborts { "abort" } else { "continue" }
            ));
            script.scheduled += usize::from(offset != 0);
            policy = Some(aborts);
        }
        let target = if command.flags & 4 != 0 {
            "source"
        } else {
            "optional-selected"
        };
        let instruction = instruction(source, command, &mut script)?;
        if command.kind == 18 && order + 1 != commands.len() {
            return Err("source-despawn-before-later-commands".into());
        }
        script.steps.push(format!(
            "{},source>{target},{instruction}",
            header(order * 2 + 1)
        ));
        script.scheduled += usize::from(offset != 0);
    }
    Ok(script)
}

fn instruction(source: &Source, c: &Command, script: &mut Script) -> Result<String, String> {
    let [a, b, d, e] = c.longs;
    let no_extra = d == 0 && e == 0;
    match c.kind {
        0 if matches!(a, 0..=4 | 6 | 7) => {
            let ids = c
                .ints
                .iter()
                .copied()
                .take_while(|id| *id != 0)
                .map(u32::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "broadcast-id")?;
            if ids.is_empty()
                || ids
                    .iter()
                    .any(|id| !source.broadcasts.get(id).is_some_and(|text| text.supported))
            {
                return Err("broadcast-dependency".into());
            }
            script.texts.extend(&ids);
            Ok(if a == 0 {
                format!("talk:{}", joined(&ids))
            } else {
                let chat = match a {
                    3 => lyracore_shared::chat::broadcast_chat::CREATURE_BOSS_EMOTE,
                    4 => lyracore_shared::chat::broadcast_chat::CREATURE_WHISPER,
                    other => other as u8,
                };
                format!("talk-as:{chat}:{}", joined(&ids))
            })
        }
        1 if a != 0 && b == 0 && no_extra && c.ints[0] == 0 => Ok(format!("emote:{a}")),
        4 if matches!(a, 46 | 147) && matches!(d, 1 | 2) => Ok(format!(
            "modify-{}-flags:{b}:{}",
            if a == 46 { "unit" } else { "npc" },
            if d == 1 { "add" } else { "remove" }
        )),
        7 if d <= 1 && e == 0 && source.quests.contains(&a) => {
            script.quests.insert(a);
            Ok(format!("creature-control:script-quest-event:{a}:{b}:{d}"))
        }
        8 if b == 0 && no_extra => Ok(format!("creature-control:kill-credit:{a}")),
        14 if b == 0 && source.spells.contains(&a) => {
            Ok(format!("creature-control:remove-aura:{a}"))
        }
        15 if no_extra && b & !(1 | 2 | 32 | 256) == 0 && source.spells.contains(&a) => {
            Ok(format!(
                "cast-options:{a}:{}:{}:{}:{}",
                u8::from(b & 2 != 0),
                u8::from(b & 1 != 0),
                u8::from(b & 32 != 0),
                u8::from(b & 256 != 0)
            ))
        }
        18 if b == 0 && no_extra => Ok(format!("despawn-source:{a}")),
        21 if a <= 1 => Ok(format!("set-active:{a}")),
        22 if a != 0 && b == 0 && no_extra => Ok(format!("set-faction:{a}:permanent")),
        25 if a <= 1 => Ok(format!("set-run:{a}")),
        26 => Ok("attack-start".into()),
        28 if a <= 8 => Ok(format!("set-stand-state:{a}")),
        42 if a <= 1 => Ok(format!("creature-control:melee:{a}")),
        43 if a <= 1 => Ok(format!("creature-control:combat-movement:{a}")),
        44 if a < 32 && b <= 2 => Ok(match b {
            0 => format!("creature-control:phase:{a}"),
            1 => format!("creature-control:phase-inc:{a}"),
            _ => format!("creature-control:phase-inc:-{a}"),
        }),
        59 if a <= 2 => Ok(format!(
            "creature-control:react-state:{}",
            ["passive", "defensive", "aggressive"][a as usize]
        )),
        39 => {
            let choices = choices(c)?;
            script.dependencies.extend(
                choices
                    .iter()
                    .copied()
                    .filter(|id| *id != 0)
                    .collect::<BTreeSet<_>>(),
            );
            Ok(format!("start-choices:{}", joined(&choices)))
        }
        _ => Err(format!("command:{}:parameters", c.kind)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eventai::vmangos::tests::{event, source, step};
    use std::collections::HashSet;

    #[test]
    fn examination_speech_and_credit_preserve_chat_type_and_delay() {
        let mut source = source();
        source.broadcasts.insert(
            55,
            super::super::Broadcast {
                encoded: String::new(),
                mode: "say",
                supported: true,
            },
        );
        let mut speech = step(0, [2, 0, 0, 0]);
        speech.ints[0] = 55;
        speech.delay = 1;
        let mut credit = step(7, [41, 20, 1, 0]);
        credit.delay = 33;
        let script = compile_script(&source, 2608, &[speech, credit]).unwrap();
        let encoded = script.encode(2608);
        assert!(encoded.contains("1000,0,1,source>optional-selected,talk-as:2:55"));
        assert!(encoded.contains(
            "33000,0,3,source>optional-selected,creature-control:script-quest-event:41:20:1"
        ));
        assert_eq!(script.quests, BTreeSet::from([41]));
    }

    #[test]
    fn weighted_starts_preserve_unused_probability_and_delayed_order() {
        let mut source = source();
        let mut start = step(39, [20, 21, 0, 0]);
        start.ints = [30, 50, 0, 0];
        source.scripts.insert(10, vec![start]);
        source.events.push(event([10, 0, 0]));
        let mut later = step(28, [8, 0, 0, 0]);
        later.delay = 2;
        later.flags = 8;
        source.delayed.insert(
            "generic_scripts",
            BTreeMap::from([
                (20, vec![step(25, [1, 0, 0, 0]), later]),
                (21, vec![step(28, [3, 0, 0, 0])]),
            ]),
        );
        let (plan, _) = source.assemble(&HashSet::from([100]), &HashSet::from([100]));
        assert_eq!(plan.relay_definition_rows.len(), 2);
        assert!(plan.relay_definition_rows[0].contains("2000,0,2,source>source,failure-policy:abort~2000,0,3,source>optional-selected,set-stand-state:8"));
        let rule = &plan.definition_rows[0];
        let weights = rule
            .split("start-script:")
            .nth(1)
            .unwrap()
            .split(':')
            .next()
            .unwrap()
            .split('.')
            .collect::<Vec<_>>();
        assert_eq!(weights.iter().filter(|id| **id == "20").count(), 30);
        assert_eq!(weights.iter().filter(|id| **id == "21").count(), 50);
        assert_eq!(weights.iter().filter(|id| **id == "0").count(), 20);
    }

    #[test]
    fn unsupported_child_rejects_its_calling_event_without_partial_scripts() {
        for change in [0, 1, 2, 3] {
            let mut source = source();
            let mut start = step(39, [20, 0, 0, 0]);
            start.ints[0] = 100;
            source.scripts.insert(10, vec![start.clone()]);
            source.events.push(event([10, 0, 0]));
            let mut child = step(28, [3, 0, 0, 0]);
            match change {
                0 => child.condition = 7,
                1 => child.target = 11,
                2 => child = start,
                _ => child = step(1, [0; 4]),
            }
            source
                .delayed
                .insert("generic_scripts", BTreeMap::from([(20, vec![child])]));
            let (plan, report) = source.assemble(&HashSet::from([100]), &HashSet::from([100]));
            assert!(plan.definition_rows.is_empty());
            assert!(plan.relay_definition_rows.is_empty());
            assert_eq!(report["counts"]["dropped_rules"], 1);
        }
    }

    #[test]
    fn repeated_child_starts_each_charge_the_expanded_work_budget() {
        let mut source = source();
        let mut start = step(39, [21, 0, 0, 0]);
        start.ints[0] = 100;
        source.delayed.insert(
            "generic_scripts",
            BTreeMap::from([
                (20, vec![start; 3]),
                (21, vec![step(28, [3, 0, 0, 0]); 1500]),
            ]),
        );
        let scripts = Scripts::compile(&source);
        assert!(scripts.closure(&[21]).is_ok());
        assert!(scripts.closure(&[20]).unwrap_err().ends_with("work-budget"));
    }
}
