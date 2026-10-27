use super::{encode_definition, pack_definition_batches, EventAiPlan, Subject};
use crate::dump::{sql_text, TableRows};
use crate::world_source::Row;
use anyhow::{ensure, Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Clone)]
struct Event {
    id: u64,
    entry: u32,
    condition: u32,
    kind: u32,
    inverse_phases: u32,
    chance: u8,
    flags: u32,
    params: [u32; 4],
    actions: [u32; 3],
}

#[derive(Clone)]
pub(super) struct Command {
    id: u32,
    delay: u32,
    priority: u32,
    order: u32,
    kind: u32,
    longs: [u32; 4],
    ints: [i32; 4],
    target: u32,
    target_params: [u32; 2],
    flags: u32,
    position: [f32; 4],
    condition: u32,
}

#[derive(Default)]
struct Compiled {
    instructions: Vec<String>,
    ends: Vec<u32>,
    aborts: Vec<u32>,
    texts: BTreeSet<u32>,
    summons: BTreeMap<u32, String>,
    templates: BTreeSet<u64>,
    quests: BTreeSet<u32>,
    limitations: BTreeSet<String>,
}

pub(crate) struct Source {
    events: Vec<Event>,
    event_ai_entries: BTreeSet<u32>,
    scripts: BTreeMap<u32, Vec<Command>>,
    broadcasts: BTreeMap<u32, Broadcast>,
    predicates: BTreeMap<u32, Option<String>>,
    quests: BTreeSet<u32>,
    spells: BTreeSet<u32>,
}

struct Broadcast {
    encoded: String,
    mode: &'static str,
    supported: bool,
}

type Findings = BTreeMap<String, BTreeSet<String>>;

fn array<T, const N: usize>(mut read: impl FnMut(usize) -> Result<T>) -> Result<[T; N]> {
    let values = (0..N).map(&mut read).collect::<Result<Vec<_>>>()?;
    Ok(values
        .try_into()
        .unwrap_or_else(|_| unreachable!("array length is fixed")))
}

fn command(row: &Row, order: u32) -> Result<Command> {
    Ok(Command {
        id: row.number("id")?,
        delay: row.number("delay")?,
        priority: row.number("priority")?,
        order,
        kind: row.number("command")?,
        longs: array(|i| {
            row.number(&if i == 0 {
                "datalong".into()
            } else {
                format!("datalong{}", i + 1)
            })
        })?,
        ints: array(|i| {
            row.number(&if i == 0 {
                "dataint".into()
            } else {
                format!("dataint{}", i + 1)
            })
        })?,
        target: row.number("target_type")?,
        target_params: [row.number("target_param1")?, row.number("target_param2")?],
        flags: row.number("data_flags")?,
        position: array(|i| row.number(["x", "y", "z", "o"][i]))?,
        condition: row.number("condition_id")?,
    })
}

fn commands(
    source: &(impl TableRows + ?Sized),
    table: &str,
) -> Result<BTreeMap<u32, Vec<Command>>> {
    let mut commands: BTreeMap<u32, Vec<Command>> = BTreeMap::new();
    for (order, row) in source.rows(table)?.iter().enumerate() {
        let command = command(row, u32::try_from(order)?)?;
        ensure!(
            command.position.iter().all(|v| v.is_finite()),
            "{table} {}: nonfinite position",
            command.id
        );
        commands.entry(command.id).or_default().push(command);
    }
    for steps in commands.values_mut() {
        steps.sort_by_key(|step| (step.delay, step.priority, step.order));
    }
    Ok(commands)
}

impl Source {
    pub(crate) fn read(source: &(impl TableRows + ?Sized)) -> Result<Self> {
        let mut events = Vec::new();
        let mut ids = BTreeSet::new();
        for row in source.rows("creature_ai_events")?.iter() {
            let event = Event::read(row)?;
            ensure!(
                event.id != 0 && ids.insert(event.id),
                "duplicate or zero EventAI id {}",
                event.id
            );
            events.push(event);
        }
        events.sort_by_key(|event| event.id);
        let mut broadcasts = BTreeMap::new();
        for row in source.rows("broadcast_text")?.iter() {
            let id = row.number("entry")?;
            let kind: u8 = row.number("chat_type")?;
            let mode = match kind {
                0 => "say",
                1 => "yell",
                2 => "emote",
                3 => "boss-emote",
                4 => "whisper",
                6 => "zone-yell",
                7 => "zone-emote",
                _ => "say",
            };
            let language: u8 = row.number("language_id")?;
            let sound: u32 = row.number("sound_id")?;
            let emotes: [(u32, u32); 3] = array(|i| {
                Ok((
                    row.number(&format!("emote_delay{}", i + 1))?,
                    row.number(&format!("emote_id{}", i + 1))?,
                ))
            })?;
            let chat = match kind {
                3 => lyracore_shared::chat::broadcast_chat::CREATURE_BOSS_EMOTE,
                4 => lyracore_shared::chat::broadcast_chat::CREATURE_WHISPER,
                other => other,
            };
            broadcasts.insert(
                id,
                Broadcast {
                    encoded: format!(
                        "({id},{},{},{chat},{language},{},{},{},{},{},{})",
                        sql_text(row.text("male_text")?),
                        sql_text(row.text("female_text")?),
                        emotes[0].0,
                        emotes[0].1,
                        emotes[1].0,
                        emotes[1].1,
                        emotes[2].0,
                        emotes[2].1
                    ),
                    mode,
                    supported: !super::fixture_id(id)
                        && matches!(kind, 0..=4 | 6 | 7)
                        && sound == 0,
                },
            );
        }
        let condition_rows = source.rows("conditions")?;
        let static_conditions = crate::source_selection::static_conditions(&condition_rows)?;
        let mut predicates = BTreeMap::from([(0, Some("always".into()))]);
        for row in condition_rows.iter() {
            let id: u32 = row.number("condition_entry")?;
            let kind: i32 = row.number("type")?;
            let values: [i64; 4] = array(|i| row.number(&format!("value{}", i + 1)))?;
            let flags: u32 = row.number("flags")?;
            let predicate = if static_conditions.get(&id) == Some(&Some(true)) {
                Some("always".into())
            } else if flags == 0 && values[1..].iter().all(|v| *v == 0) {
                match (kind, values[0]) {
                    (6, 469) => Some("alliance".into()),
                    (6, 67) => Some("horde".into()),
                    (9, quest) if quest > 0 && quest <= i64::from(u32::MAX) => {
                        Some(format!("quest-taken.{quest}"))
                    }
                    _ => None,
                }
            } else {
                None
            };
            predicates.insert(id, predicate);
        }
        Ok(Self {
            events,
            event_ai_entries: source
                .columns("creature_template", &["entry", "ai_name"])?
                .iter()
                .filter_map(|row| match row.text("ai_name") {
                    Ok("EventAI") => Some(row.number("entry")),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<_>>()?,
            scripts: commands(source, "creature_ai_scripts")?,
            broadcasts,
            predicates,
            quests: source
                .columns("quest_template", &["entry"])?
                .iter()
                .map(|r| r.number("entry"))
                .collect::<Result<_>>()?,
            spells: source
                .columns("spell_template", &["entry"])?
                .iter()
                .map(|r| r.number("entry"))
                .collect::<Result<_>>()?,
        })
    }

    pub(crate) fn assemble(
        &self,
        entries: &HashSet<u64>,
        templates: &HashSet<u64>,
    ) -> (EventAiPlan, serde_json::Value) {
        let mut plan = EventAiPlan::default();
        let mut definitions: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        let mut findings = Findings::new();
        let mut used_texts = BTreeSet::new();
        let mut summons = BTreeMap::new();
        let mut limitations = Findings::new();
        let mut emitted = 0;
        let mut excluded = 0;
        for event in &self.events {
            if !entries.contains(&u64::from(event.entry))
                || !self.event_ai_entries.contains(&event.entry)
                || event.flags & 16 != 0
            {
                excluded += 1;
                continue;
            }
            match self.compile_event(event, templates) {
                Ok((rule, compiled)) => {
                    definitions.entry(event.entry).or_default().push(rule);
                    plan.coverage.emitted_instructions += compiled.instructions.len() as u64;
                    used_texts.extend(compiled.texts);
                    summons.extend(compiled.summons);
                    plan.forced_template_entries.extend(compiled.templates);
                    plan.quest_event_entries.extend(compiled.quests);
                    for limitation in compiled.limitations {
                        limitations
                            .entry(limitation)
                            .or_default()
                            .insert(format!("event:{}:creature:{}", event.id, event.entry));
                    }
                    emitted += 1;
                }
                Err(reason) => {
                    findings
                        .entry(reason)
                        .or_default()
                        .insert(format!("event:{}:creature:{}", event.id, event.entry));
                }
            }
        }
        plan.definition_rows = definitions
            .into_iter()
            .map(|(entry, rules)| encode_definition(Subject::Entry(entry), &rules))
            .collect();
        plan.definition_batches = pack_definition_batches(&plan.definition_rows);
        plan.broadcast_rows = used_texts
            .into_iter()
            .map(|id| self.broadcasts[&id].encoded.clone())
            .collect();
        plan.summon_rows = summons.into_values().collect();
        plan.relay_definition_batches = vec![String::new()];
        let manifest = serde_json::json!({
            "format_version": 1, "source": "vmangos", "family": "creature-ai",
            "counts": { "source_rules": self.events.len(), "emitted_rules": emitted, "excluded_rules": excluded, "dropped_rules": self.events.len() - emitted - excluded, "definitions": plan.definition_rows.len(), "emitted_instructions": plan.instruction_count(), "relays": 0 },
            "source_census": { "events": census(self.events.iter().map(|e| e.kind)), "event_flags": census(self.events.iter().map(|e| e.flags)), "commands": census(self.scripts.values().flatten().map(|c| c.kind)), "targets": census(self.scripts.values().flatten().map(|c| c.target)), "command_flags": census(self.scripts.values().flatten().map(|c| c.flags)) },
            "limitations": limitations.into_iter().map(|(code, source_ids)| serde_json::json!({"code":format!("eventai.{code}"),"classification":"unsupported","occurrences":source_ids.len(),"source_ids":source_ids})).collect::<Vec<_>>(),
            "findings": findings.into_iter().map(|(code, source_ids)| serde_json::json!({"code":format!("eventai.{code}"),"classification":"unsupported","occurrences":source_ids.len(),"source_ids":source_ids})).collect::<Vec<_>>()
        });
        (plan, manifest)
    }

    fn compile_event(
        &self,
        event: &Event,
        templates: &HashSet<u64>,
    ) -> std::result::Result<(String, Compiled), String> {
        if event.flags & !31 != 0
            || event.chance == 0
            || event.chance > 100
            || event.inverse_phases == u32::MAX
        {
            return Err("event-policy".into());
        }
        if !templates.contains(&u64::from(event.entry)) || super::fixture_id(event.entry) {
            return Err("subject".into());
        }
        let predicate = self
            .predicates
            .get(&event.condition)
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("condition:{}", event.condition))?;
        let (trigger, recurrence) = event.compile()?;
        let mut compiled = Compiled::default();
        for id in event.actions.into_iter().filter(|id| *id != 0) {
            let commands = self
                .scripts
                .get(&id)
                .ok_or_else(|| format!("missing-command-list:{id}"))?;
            for command in commands {
                if command.delay != 0 {
                    return Err("event-command-delay".into());
                }
                if command.condition != 0
                    && self
                        .predicates
                        .get(&command.condition)
                        .and_then(Option::as_deref)
                        != Some("always")
                {
                    return Err(format!("command-condition:{}", command.condition));
                }
                let instruction = self.compile_command(command, &mut compiled, templates)?;
                if command.flags & 8 != 0 {
                    compiled.aborts.push(compiled.instructions.len() as u32);
                }
                compiled.instructions.push(instruction);
            }
            compiled.ends.push(compiled.instructions.len() as u32);
        }
        if compiled.instructions.is_empty()
            || compiled.instructions.len() > super::RELAY_STEP_BUDGET
        {
            return Err("command-list-budget".into());
        }
        let rule = format!(
            "{},{},{},{},{},lists:{}:{}:{},script:{}:{}:{},{}",
            event.id,
            trigger,
            event.chance,
            !event.inverse_phases,
            recurrence,
            u8::from(event.flags & 2 != 0),
            joined(&compiled.ends),
            joined(&compiled.aborts),
            u8::from(event.flags & 4 != 0),
            u8::from(event.flags & 8 != 0),
            predicate,
            compiled.instructions.join("+")
        );
        Ok((rule, compiled))
    }

    fn compile_command(
        &self,
        command: &Command,
        compiled: &mut Compiled,
        templates: &HashSet<u64>,
    ) -> std::result::Result<String, String> {
        let c = command;
        let [a, b, d, e] = c.longs;
        let unsupported = || format!("command:{}:parameters", c.kind);
        if c.flags & !31 != 0 {
            return Err("command-flags".into());
        }
        if c.flags & 3 != 0 {
            return Err(format!("command:{}:source-target-swap", c.kind));
        }
        let target = c.target()?;
        let actor = c.target == 0 && c.flags & 3 == 0;
        let no_extra = d == 0 && e == 0;
        match c.kind {
            0 if a == 0 => {
                let ids = c
                    .ints
                    .iter()
                    .copied()
                    .take_while(|id| *id != 0)
                    .map(u32::try_from)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(|_| unsupported())?;
                let Some(first) = ids.first() else {
                    return Err(unsupported());
                };
                if ids
                    .iter()
                    .any(|id| !self.broadcasts.get(id).is_some_and(|text| text.supported))
                {
                    return Err("broadcast-dependency".into());
                }
                compiled.texts.extend(&ids);
                Ok(format!(
                    "speak:{}:{target}:{}",
                    self.broadcasts[first].mode,
                    joined(&ids)
                ))
            }
            1 if actor && c.ints[0] == 0 => {
                let count = 1 + c.longs[1..].iter().take_while(|id| **id != 0).count();
                if c.longs[..count].iter().any(|id| *id > i32::MAX as u32) {
                    return Err(unsupported());
                }
                Ok(if count == 1 {
                    format!("emote:{a}:self")
                } else {
                    format!("random-emote:{}", joined(&c.longs[..count]))
                })
            }
            2 if actor => match (a, b) {
                (147, 0) => Ok("npc-flags:clear".into()),
                (147, 3) => Ok("npc-flags:gossip-and-quest".into()),
                (23, 0) => Ok("mana:empty".into()),
                (37, 0) => Ok("virtual-main-hand:clear".into()),
                _ => Err(unsupported()),
            },
            4 if actor && a == 46 && matches!(d, 1 | 2) => {
                let flag = match b {
                    2 => "not-attackable",
                    256 => "immune-to-players",
                    512 => "immune-to-creatures",
                    768 => "immune-to-players-and-creatures",
                    33554432 if d == 1 => "not-selectable",
                    _ => return Err(unsupported()),
                };
                Ok(format!(
                    "unit-flags:{}:{flag}",
                    if d == 1 { "set" } else { "clear" }
                ))
            }
            7 if c.target == 0
                && c.flags & 7 == 0
                && b == 0
                && no_extra
                && self.quests.contains(&a) =>
            {
                compiled.quests.insert(a);
                Ok(format!("quest-event:{a}:selected-character"))
            }
            8 if c.target == 0 && c.flags & 7 == 0 && b == 0 && no_extra => {
                Ok(format!("kill-credit:{a}:selected-character"))
            }
            10 if actor
                && a != 0
                && no_extra
                && c.ints[0..2] == [0, 0]
                && matches!(c.ints[3], 1 | 2 | 4)
                && !super::fixture_id(a)
                && templates.contains(&u64::from(a)) =>
            {
                compile_summon(c, compiled)
            }
            14 if actor && b == 0 && self.spells.contains(&a) => {
                Ok(format!("remove-aura:{a}:self"))
            }
            15 if no_extra && b & !(1 | 2 | 32 | 256) == 0 && self.spells.contains(&a) => {
                Ok(format!(
                    "cast:{a}:{target}:{}:{}:actor:selected:{}:0:{}:0:0",
                    u8::from(b & 1 != 0),
                    if b & 2 != 0 { "triggered" } else { "direct" },
                    u8::from(b & 32 != 0),
                    u8::from(b & 256 != 0)
                ))
            }
            18 if actor && b == 0 && no_extra => Ok(format!("force-despawn:{a}")),
            22 if actor && a != 0 && b == 0 && no_extra => Ok(format!("faction:{a}")),
            23 if actor
                && a != 0
                && b == 0
                && !super::fixture_id(a)
                && templates.contains(&u64::from(a)) =>
            {
                compiled.templates.insert(u64::from(a));
                Ok(format!("display-template:{a}"))
            }
            25 if actor && a <= 1 => Ok(format!(
                "walking:{}",
                if a == 1 {
                    "run-default"
                } else {
                    "walk-default"
                }
            )),
            26 => Ok(format!("attack-start:{target}")),
            28 if actor && a <= 8 => Ok(format!("set-stand-state:{a}")),
            33 if actor => Ok("evade:0".into()),
            42 if actor && a <= 1 => Ok(format!("auto-attack:{a}")),
            43 if actor && a <= 1 => Ok(format!("combat-movement:{a}")),
            44 if actor && a < 32 && (b == 0 || b == 2 || (b == 1 && a == 0)) => Ok(match b {
                0 => format!("phase:{a}"),
                1 => format!("phase-inc:{a}"),
                _ => format!("phase-inc:-{a}"),
            }),
            45 if actor && c.longs.iter().all(|v| *v < 32) => {
                let count = if e != 0 {
                    4
                } else if d != 0 {
                    3
                } else {
                    2
                };
                Ok(format!("phase-random:{}", joined(&c.longs[..count])))
            }
            46 if actor && a <= b && b < 32 => Ok(if a == b {
                format!("phase:{a}")
            } else {
                format!("phase-range:{a}:{b}")
            }),
            47 if actor && a == 0 => {
                compiled.limitations.insert("flee-fixed-duration".into());
                if !self
                    .broadcasts
                    .get(&1150)
                    .is_some_and(|text| text.supported)
                {
                    return Err("broadcast-dependency".into());
                }
                compiled.texts.insert(1150);
                compiled
                    .instructions
                    .push("speak:emote:opponent:1150".into());
                Ok("flee".into())
            }
            49 if actor && a == 1 => Ok("zone-combat".into()),
            50 if actor && c.position[0] >= 0.0 && c.position[0].fract() == 0.0 => {
                Ok(format!("help:{}", c.position[0]))
            }
            51 if actor && a <= 2 => Ok(format!("sheath:{a}")),
            52 if actor && b == 0 && a <= 1 => Ok(format!(
                "lethal-floor:{}",
                if a == 1 { "on" } else { "off" }
            )),
            56 if actor => Ok(format!("remove-guardians:{a}")),
            59 if actor && a <= 2 => Ok(format!(
                "set-react-state:{}",
                ["passive", "defensive", "aggressive"][a as usize]
            )),
            _ => Err(unsupported()),
        }
    }
}

fn compile_summon(
    command: &Command,
    compiled: &mut Compiled,
) -> std::result::Result<String, String> {
    let [entry, duration, _, _] = command.longs;
    let unsupported = || format!("command:{}:parameters", command.kind);
    let attack = match command.ints[2] {
        -1 => "self",
        0 => command.target()?,
        1 => "opponent",
        2 => "second-threat",
        4 => "random-threat",
        5 => "random-threat-except-highest",
        7 => "farthest-hostile",
        _ => return Err(unsupported()),
    };
    compiled.templates.insert(u64::from(entry));
    if command.ints[3] != 2 {
        compiled.limitations.insert("summon-corpse-lifetime".into());
    }
    if command.position[..3] == [0.0; 3] {
        Ok(format!("spawn-at-actor:{entry}:{attack}:{duration}"))
    } else {
        let id = command.order.checked_add(1).ok_or_else(unsupported)?;
        let [x, y, z, o] = command.position;
        compiled
            .summons
            .insert(id, format!("({id},{x},{y},{z},{o},{duration})"));
        Ok(format!("summon:{entry}:{id}:{attack}"))
    }
}

impl Command {
    fn target(&self) -> std::result::Result<&'static str, String> {
        if self.target_params != [0, 0] {
            return Err(format!("target:{}:parameters", self.target));
        }
        if self.flags & 4 != 0 {
            if self.target != 0 {
                return Err("target:self-with-selection".into());
            }
            return Ok("self");
        }
        match self.target {
            0 => Ok("provided"),
            1 => Ok("opponent"),
            2 => Ok("second-threat"),
            4 => Ok("random-threat"),
            5 => Ok("random-threat-except-highest"),
            7 => Ok("farthest-hostile"),
            _ => Err(format!("target:{}", self.target)),
        }
    }
}

impl Event {
    fn read(row: &Row) -> Result<Self> {
        Ok(Self {
            id: row.number("id")?,
            entry: row.number("creature_id")?,
            condition: row.number("condition_id")?,
            kind: row.number("event_type")?,
            inverse_phases: row.number("event_inverse_phase_mask")?,
            chance: row.number("event_chance")?,
            flags: row.number("event_flags")?,
            params: array(|i| {
                let column = format!("event_param{}", i + 1);
                let value: i64 = row.number(&column)?;
                if i == 1 && matches!(row.number::<u32>("event_type")?, 8 | 36) && value == -1 {
                    Ok(u32::MAX)
                } else {
                    u32::try_from(value).with_context(|| {
                        format!("creature_ai_events.{column}: invalid unsigned parameter")
                    })
                }
            })?,
            actions: array(|i| row.number(&format!("action{}_script", i + 1)))?,
        })
    }

    fn compile(&self) -> std::result::Result<(String, String), String> {
        let [a, b, c, d] = self.params;
        let unsupported = || format!("event:{}:parameters", self.kind);
        let percentage = |name| {
            if a <= 100 && b <= a {
                Ok(format!("{name}:{b}:{a}"))
            } else {
                Err(unsupported())
            }
        };
        let trigger = match self.kind {
            0 | 1 if a <= b => format!(
                "{}:{a}:{b}",
                if self.kind == 0 {
                    "timer-combat"
                } else {
                    "timer-ooc"
                }
            ),
            2 => percentage("health")?,
            3 => percentage("power")?,
            4 => "aggro".into(),
            5 if c <= 1 => format!("kill:{c}"),
            6 => "death:always".into(),
            7 => "evade".into(),
            8 if b != 0 => format!("spell-hit:{a}:{b}"),
            9 if a <= b => format!("range:{a}:{b}"),
            10 if matches!(a, 1 | 2) => format!("ooc-los:{}:{b}:0:always", a - 1),
            11 => "spawn:always".into(),
            12 => percentage("target-health")?,
            13 => "target-casting".into(),
            14 => format!("friendly-health:{a}:{b}:0"),
            15 if a == 0 => format!("friendly-cc:{b}"),
            16 => format!("friendly-missing-aura:{a}:{b}:nearby-engaged"),
            17 => format!("summoned:{a}"),
            18 => percentage("target-power")?,
            21 => "home".into(),
            22 => format!("receive-emote:{a}:always"),
            23 => format!("aura:{a}:{b}"),
            24 => format!("target-aura:{a}:{b}"),
            25 => format!("summoned-death:{a}"),
            27 => format!("missing-aura:{a}:{b}"),
            28 => format!("target-missing-aura:{a}:{b}"),
            36 if b != 0 => format!("spell-hit-target:{a}:{b}"),
            _ => return Err(unsupported()),
        };
        let recurrence = if self.flags & 1 == 0 {
            "once".into()
        } else {
            let window = match self.kind {
                5 | 13 => Some((a, b)),
                17 | 25 => Some((b, c)),
                0..=3 | 8..=10 | 12 | 14..=16 | 18 | 23..=24 | 27..=28 | 36 => Some((c, d)),
                _ => None,
            };
            match window {
                Some((min, max)) if min <= max => format!("repeat:{min}:{max}"),
                None if matches!(self.kind, 11 | 22) => "repeat-event".into(),
                _ => return Err(unsupported()),
            }
        };
        Ok((trigger, recurrence))
    }
}

fn joined(values: &[u32]) -> String {
    values
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

fn census(values: impl Iterator<Item = u32>) -> BTreeMap<u32, usize> {
    let mut counts = BTreeMap::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_source::{sqlite_fixture, FixtureTable};
    use rusqlite::types::Value;

    fn source() -> Source {
        Source {
            events: Vec::new(),
            event_ai_entries: BTreeSet::from([100]),
            scripts: BTreeMap::new(),
            broadcasts: BTreeMap::new(),
            predicates: BTreeMap::from([(0, Some("always".into()))]),
            quests: BTreeSet::from([41]),
            spells: BTreeSet::from([42]),
        }
    }

    fn step(kind: u32, longs: [u32; 4]) -> Command {
        Command {
            id: 10,
            delay: 0,
            priority: 0,
            order: 0,
            kind,
            longs,
            ints: [0; 4],
            target: 0,
            target_params: [0; 2],
            flags: 0,
            position: [0.0; 4],
            condition: 0,
        }
    }

    fn event(actions: [u32; 3]) -> Event {
        Event {
            id: 1,
            entry: 100,
            condition: 0,
            kind: 4,
            inverse_phases: 0,
            chance: 100,
            flags: 0,
            params: [0; 4],
            actions,
        }
    }

    #[test]
    fn named_command_reader_orders_priority_ascending_and_preserves_ties() {
        let columns = "id delay priority command datalong datalong2 datalong3 datalong4 dataint dataint2 dataint3 dataint4 target_type target_param1 target_param2 data_flags condition_id x y z o"
            .split_whitespace().rev().map(|column| (column, "INTEGER NOT NULL DEFAULT 0")).collect::<Vec<_>>();
        let rows = [
            vec![
                ("id", Value::Integer(10)),
                ("priority", Value::Integer(2)),
                ("datalong", Value::Integer(3)),
            ],
            vec![
                ("id", Value::Integer(10)),
                ("priority", Value::Integer(1)),
                ("datalong", Value::Integer(1)),
            ],
            vec![
                ("id", Value::Integer(10)),
                ("priority", Value::Integer(1)),
                ("datalong", Value::Integer(2)),
            ],
        ];
        let row_refs = rows.iter().map(Vec::as_slice).collect::<Vec<_>>();
        let database = sqlite_fixture(&[FixtureTable {
            name: "creature_ai_scripts",
            columns: &columns,
            key: &[],
            rows: &row_refs,
        }])
        .unwrap();
        let result = commands(&database, "creature_ai_scripts").unwrap();
        assert_eq!(
            result[&10]
                .iter()
                .map(|step| step.longs[0])
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn command_lists_keep_boundaries_random_selection_abort_bits_and_event_flags() {
        let mut source = source();
        let mut aborting = step(44, [1, 0, 0, 0]);
        aborting.flags = 8;
        source
            .scripts
            .insert(10, vec![aborting, step(44, [2, 0, 0, 0])]);
        source.scripts.insert(11, vec![step(44, [3, 0, 0, 0])]);
        let mut event = event([10, 11, 0]);
        event.flags = 2 | 4 | 8;
        let (rule, _) = source.compile_event(&event, &HashSet::from([100])).unwrap();
        assert_eq!(
            rule,
            "1,aggro,100,4294967295,once,lists:1:2.3:0,script:1:1:always,phase:1+phase:2+phase:3"
        );
    }

    #[test]
    fn unsupported_target_or_command_condition_drops_the_whole_rule() {
        let mut source = source();
        for rejected in [
            Command {
                target: 11,
                target_params: [17, 0],
                ..step(15, [42, 0, 0, 0])
            },
            Command {
                condition: 7,
                ..step(44, [2, 0, 0, 0])
            },
            Command {
                flags: 2,
                ..step(15, [42, 0, 0, 0])
            },
        ] {
            source
                .scripts
                .insert(10, vec![step(44, [1, 0, 0, 0]), rejected]);
            source.events = vec![event([10, 0, 0])];
            let (plan, manifest) = source.assemble(&HashSet::from([100]), &HashSet::from([100]));
            assert!(plan.definition_rows.is_empty());
            assert_eq!(manifest["counts"]["dropped_rules"], 1);
        }
    }

    #[test]
    fn spell_school_wildcard_is_checked_separately_from_unsigned_parameters() {
        let columns = "id creature_id condition_id event_type event_inverse_phase_mask event_chance event_flags event_param1 event_param2 event_param3 event_param4 action1_script action2_script action3_script"
            .split_whitespace().map(|column| (column, "INTEGER NOT NULL DEFAULT 0")).collect::<Vec<_>>();
        for (kind, parameter, accepted) in [
            (8, -1, true),
            (36, -1, true),
            (8, -2, false),
            (0, -1, false),
        ] {
            let source = sqlite_fixture(&[FixtureTable {
                name: "creature_ai_events",
                columns: &columns,
                key: &[],
                rows: &[&[
                    ("event_type", Value::Integer(kind)),
                    ("event_param2", Value::Integer(parameter)),
                ]],
            }])
            .unwrap();
            let rows = source.rows("creature_ai_events").unwrap();
            let read = Event::read(&rows[0]);
            assert_eq!(read.is_ok(), accepted);
            if accepted {
                assert_eq!(read.unwrap().params[1], u32::MAX);
            }
        }
    }

    #[test]
    fn source_event_numbers_do_not_inherit_classic_meanings() {
        let mut event = event([10, 0, 0]);
        event.kind = 29;
        assert!(event.compile().is_err());
        event.kind = 36;
        event.params = [42, u32::MAX, 0, 0];
        assert_eq!(event.compile().unwrap().0, "spell-hit-target:42:4294967295");
    }

    #[test]
    fn random_phase_zero_is_a_choice_only_in_the_first_two_slots() {
        let source = source();
        let mut compiled = Compiled::default();
        let templates = HashSet::new();
        for (longs, expected) in [
            ([0, 2, 0, 0], "phase-random:0.2"),
            ([1, 2, 3, 0], "phase-random:1.2.3"),
            ([1, 2, 0, 4], "phase-random:1.2.0.4"),
        ] {
            assert_eq!(
                source
                    .compile_command(&step(45, longs), &mut compiled, &templates)
                    .unwrap(),
                expected
            );
        }
        assert_eq!(
            source
                .compile_command(&step(46, [3, 3, 0, 0]), &mut compiled, &templates)
                .unwrap(),
            "phase:3"
        );
    }

    #[test]
    fn display_templates_are_dependencies_and_fixture_entries_are_refused() {
        let mut source = source();
        source.scripts.insert(10, vec![step(23, [101, 0, 0, 0])]);
        source.events = vec![event([10, 0, 0])];
        let (plan, _) = source.assemble(&HashSet::from([100]), &HashSet::from([100, 101]));
        assert_eq!(plan.forced_template_entries, BTreeSet::from([101]));
        source.events[0].entry = 5_099_001;
        source.event_ai_entries.insert(5_099_001);
        let (plan, manifest) = source.assemble(
            &HashSet::from([5_099_001]),
            &HashSet::from([5_099_001, 101]),
        );
        assert!(plan.definition_rows.is_empty());
        assert_eq!(manifest["counts"]["dropped_rules"], 1);
    }
    #[test]
    fn flee_keeps_the_source_emote_in_the_same_command_list() {
        let mut source = source();
        source.broadcasts.insert(
            1150,
            Broadcast {
                encoded: "(1150,'flee','',2,0,0,0,0,0,0,0)".into(),
                mode: "emote",
                supported: true,
            },
        );
        source.scripts.insert(10, vec![step(47, [0; 4])]);
        source.events.push(event([10, 0, 0]));
        let (plan, _) = source.assemble(&HashSet::from([100]), &HashSet::from([100]));
        assert!(plan.definition_rows[0].contains("lists:0:2:"));
        assert!(plan.definition_rows[0].ends_with("speak:emote:opponent:1150+flee"));
        assert_eq!(plan.broadcast_rows.len(), 1);
    }

    #[test]
    fn summons_keep_their_position_attack_target_and_template_dependency() {
        let mut source = source();
        let mut summon = step(10, [101, 30000, 0, 0]);
        summon.ints = [0, 0, 1, 1];
        source.events.push(event([10, 0, 0]));
        for (position, instruction) in [
            ([0.0; 4], "spawn-at-actor:101:opponent:30000"),
            ([2.0, 3.0, 4.0, 1.0], "summon:101:1:opponent"),
        ] {
            summon.position = position;
            source.scripts.insert(10, vec![summon.clone()]);
            let (plan, _) = source.assemble(&HashSet::from([100]), &HashSet::from([100, 101]));
            assert!(plan.definition_rows[0].ends_with(instruction));
            assert_eq!(plan.forced_template_entries, BTreeSet::from([101]));
            if position[0] != 0.0 {
                assert_eq!(plan.summon_rows, ["(1,2,3,4,1,30000)"]);
            }
        }
        summon.position = [0.0; 4];
        summon.ints[2] = 0;
        summon.flags = 4;
        source.scripts.insert(10, vec![summon.clone()]);
        let (plan, _) = source.assemble(&HashSet::from([100]), &HashSet::from([100, 101]));
        assert!(plan.definition_rows[0].ends_with("spawn-at-actor:101:self:30000"));
        summon.longs[2] = 6;
        source.scripts.insert(10, vec![summon]);
        let (plan, _) = source.assemble(&HashSet::from([100]), &HashSet::from([100, 101]));
        assert!(plan.definition_rows.is_empty());
    }

    #[test]
    fn random_emotes_keep_only_the_contiguous_source_choices() {
        let source = source();
        for (choices, expected) in [
            ([18, 20, 0, 0], "random-emote:18.20"),
            ([4, 21, 22, 11], "random-emote:4.21.22.11"),
            ([4, 0, 22, 11], "emote:4:self"),
        ] {
            assert_eq!(
                source
                    .compile_command(&step(1, choices), &mut Compiled::default(), &HashSet::new())
                    .unwrap(),
                expected
            );
        }
        let mut targeted = step(1, [4, 21, 0, 0]);
        targeted.ints[0] = 1;
        assert!(source
            .compile_command(&targeted, &mut Compiled::default(), &HashSet::new())
            .is_err());
    }
}
