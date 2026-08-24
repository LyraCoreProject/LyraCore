use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{game_creature_ai_broadcast_text, game_world_entity};

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayConcurrency {
    Parallel,
    IgnoreIfRunning,
    Replace,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayLifetime {
    MapOrInstance,
    SourceCreature,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelaySubject {
    Source,
    Selected,
    NearbyCreature(RelayNearby),
    NearbyGameObject(RelayNearby),
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelayParticipants {
    pub source: RelaySubject,
    pub target: RelaySubject,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelayNearby {
    pub entry: u32,
    pub radius_yd: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub struct RelayTalk {
    pub broadcast_ids: Vec<u32>,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayEmote {
    pub emote_id: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayMoveTo {
    pub travel_ms: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelaySpawnCreature {
    pub entry: u32,
    pub despawn_ms: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayCastSpell {
    pub spell_id: u32,
    pub triggered: bool,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayDelay {
    pub delay_ms: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayMovement {
    pub movement_kind: u8,
    pub distance_yd: u32,
    pub path_id: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayEnabled {
    pub enabled: bool,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayFaction {
    pub faction_template: u32,
    pub temporary: bool,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayStandState {
    pub stand_state: u8,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayFlagChange {
    pub flags: u32,
    pub add: bool,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayAiEvent {
    pub kind: super::AiEventKind,
    pub radius_yd: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayFacing {
    pub orientation: f32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayDynamicMove {
    pub travel_ms: u32,
    pub distance_yd: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayEquipment {
    pub main_hand: i32,
    pub off_hand: i32,
    pub ranged: i32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayTemplateUpdate {
    pub entry: u32,
    pub team: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayGossipMenu {
    pub menu_id: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayWorldState {
    pub world_state_id: u32,
    pub value: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayStart {
    pub relay_id: u32,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub enum RelayInstruction {
    Talk(RelayTalk),
    Emote(RelayEmote),
    MoveTo(RelayMoveTo),
    SpawnCreature(RelaySpawnCreature),
    ActivateObject,
    CastSpell(RelayCastSpell),
    DespawnSource(RelayDelay),
    SetMovement(RelayMovement),
    SetActive(RelayEnabled),
    SetFaction(RelayFaction),
    SetRun(RelayEnabled),
    AttackStart,
    SetStandState(RelayStandState),
    ModifyNpcFlags(RelayFlagChange),
    TerminateInvocation,
    PauseWaypoints(RelayEnabled),
    SendAiEvent(RelayAiEvent),
    SetFacing(RelayFacing),
    MoveDynamic(RelayDynamicMove),
    DespawnGameObject(RelayDelay),
    SetEquipment(RelayEquipment),
    UpdateCreatureTemplate(RelayTemplateUpdate),
    ModifyUnitFlags(RelayFlagChange),
    SetGossipMenu(RelayGossipMenu),
    SetWorldState(RelayWorldState),
    StartRelay(RelayStart),
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub struct RelayStep {
    pub offset_ms: u32,
    pub command_priority: u32,
    pub source_order: u32,
    pub participants: RelayParticipants,
    pub instruction: RelayInstruction,
}

#[table(
    accessor = game_creature_ai_relay_definition,
    index(accessor = by_relay, btree(columns = [relay_id]))
)]
pub struct RelayDefinition {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub relay_id: u32,
    pub version: u64,
    pub catalogue_version: u64,
    pub current: bool,
    pub concurrency: RelayConcurrency,
    pub lifetime: RelayLifetime,
    pub steps: Vec<RelayStep>,
}

#[table(
    accessor = game_creature_ai_relay_run,
    index(accessor = by_instance, btree(columns = [instance_id])),
    index(accessor = by_source, btree(columns = [source_guid]))
)]
pub struct RelayRun {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub relay_id: u32,
    pub definition_id: u64,
    pub definition_version: u64,
    pub catalogue_version: u64,
    pub next_step: u32,
    pub started_at: Timestamp,
    pub due_at: Timestamp,
    pub map_id: u32,
    pub instance_id: u64,
    pub source_guid: u64,
    pub selected_guid: u64,
    pub saved_random_state: u64,
    pub concurrency: RelayConcurrency,
    pub lifetime: RelayLifetime,
    pub parent_run_id: u64,
}

#[table(
    accessor = game_creature_ai_relay_continuation,
    index(accessor = by_run, btree(columns = [run_id])),
    scheduled(resume_relay_run)
)]
pub struct RelayContinuation {
    #[primary_key]
    #[auto_inc]
    pub scheduled_id: u64,
    pub scheduled_at: ScheduleAt,
    pub run_id: u64,
}

#[reducer]
pub fn import_creature_ai_relay_definitions(
    ctx: &ReducerContext,
    packed: String,
) -> Result<(), String> {
    crate::helpers::require_operator(ctx)?;
    load_definition_catalogue(ctx, &packed)
}

fn load_definition_catalogue(ctx: &ReducerContext, packed: &str) -> Result<(), String> {
    let mut definitions = packed
        .lines()
        .filter(|line| !line.is_empty())
        .map(parse_definition)
        .collect::<Result<Vec<_>, _>>()?;
    let table = ctx.db.game_creature_ai_relay_definition();
    let mut graph = std::collections::BTreeMap::new();
    for definition in &definitions {
        if graph
            .insert(definition.relay_id, definition.steps.clone())
            .is_some()
        {
            return Err(format!(
                "relay definition {} appears twice in the candidate graph",
                definition.relay_id
            ));
        }
    }
    validate_definition_graph(&graph)?;
    validate_definition_dependencies(ctx, &definitions)?;
    let catalogue_version = encoded_catalogue_version(&definitions);
    for definition in &mut definitions {
        definition.catalogue_version = catalogue_version;
    }
    for mut definition in table.iter().filter(|row| row.current).collect::<Vec<_>>() {
        definition.current = false;
        table.id().update(definition);
    }
    for definition in definitions {
        if table
            .by_relay()
            .filter(&definition.relay_id)
            .any(|row| row.current)
        {
            return Err(format!(
                "relay definition {} already has a current version",
                definition.relay_id
            ));
        }
        table.insert(definition);
    }
    reap_unused_definitions(ctx);
    Ok(())
}

fn validate_definition_dependencies(
    ctx: &ReducerContext,
    definitions: &[RelayDefinition],
) -> Result<(), String> {
    let broadcasts = ctx.db.game_creature_ai_broadcast_text();
    for definition in definitions {
        for step in &definition.steps {
            let RelayInstruction::Talk(talk) = &step.instruction else {
                continue;
            };
            for broadcast_id in &talk.broadcast_ids {
                if broadcasts.id().find(broadcast_id).is_none() {
                    return Err(format!(
                        "relay dependency path {} -> row:{} -> broadcast_text:{} is missing",
                        definition.relay_id, step.source_order, broadcast_id
                    ));
                }
            }
        }
    }
    Ok(())
}

fn validate_definition_graph(
    graph: &std::collections::BTreeMap<u32, Vec<RelayStep>>,
) -> Result<(), String> {
    fn visit(
        graph: &std::collections::BTreeMap<u32, Vec<RelayStep>>,
        relay_id: u32,
        path: &mut Vec<u32>,
        step_count: &mut usize,
        scheduled_count: &mut usize,
    ) -> Result<(), String> {
        if path.contains(&relay_id) {
            path.push(relay_id);
            return Err(format!(
                "relay cycle: {}",
                path.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(" -> ")
            ));
        }
        if path.len() >= 16 {
            return Err(format!("relay depth exceeds 16 at {relay_id}"));
        }
        let steps = graph
            .get(&relay_id)
            .ok_or_else(|| format!("relay dependency {relay_id} is missing"))?;
        path.push(relay_id);
        for step in steps {
            validate_step_capability(relay_id, step)?;
            *step_count += 1;
            *scheduled_count += usize::from(step.offset_ms != 0);
            if *step_count > 4_096 {
                return Err("relay graph exceeds the 4096-step budget".to_string());
            }
            if *scheduled_count > 2_048 {
                return Err("relay graph exceeds the 2048 scheduled-work budget".to_string());
            }
            if let RelayInstruction::StartRelay(start) = &step.instruction {
                visit(graph, start.relay_id, path, step_count, scheduled_count)?;
            }
        }
        path.pop();
        Ok(())
    }

    for relay_id in graph.keys().copied() {
        let mut path = Vec::new();
        let mut step_count = 0;
        let mut scheduled_count = 0;
        visit(
            graph,
            relay_id,
            &mut path,
            &mut step_count,
            &mut scheduled_count,
        )?;
    }
    Ok(())
}

fn validate_step_capability(relay_id: u32, step: &RelayStep) -> Result<(), String> {
    let participants = [step.participants.source, step.participants.target];
    if participants.iter().any(|subject| {
        matches!(
            subject,
            RelaySubject::NearbyCreature(_) | RelaySubject::NearbyGameObject(_)
        )
    }) {
        return Err(format!(
            "relay {relay_id} step {} needs an unavailable nearby-subject resolver",
            step.source_order
        ));
    }
    match &step.instruction {
        RelayInstruction::Talk(talk) if talk.broadcast_ids.is_empty() => Err(format!(
            "relay {relay_id} step {} has no broadcast text",
            step.source_order
        )),
        RelayInstruction::Talk(_)
        | RelayInstruction::Emote(_)
        | RelayInstruction::TerminateInvocation
        | RelayInstruction::StartRelay(_) => Ok(()),
        instruction => Err(format!(
            "relay {relay_id} step {} has no gameplay authority binding for {instruction:?}",
            step.source_order
        )),
    }
}

fn parse_definition(line: &str) -> Result<RelayDefinition, String> {
    let fields = line.splitn(5, '@').collect::<Vec<_>>();
    if fields.len() != 5 {
        return Err(format!("relay definition needs five fields: {line}"));
    }
    let relay_id = parse_u32(fields[0])?;
    let version = parse_u64(fields[1])?;
    let concurrency = match fields[2] {
        "parallel" => RelayConcurrency::Parallel,
        "ignore-if-running" => RelayConcurrency::IgnoreIfRunning,
        "replace" => RelayConcurrency::Replace,
        value => return Err(format!("unknown relay concurrency: {value}")),
    };
    let lifetime = match fields[3] {
        "map-or-instance" => RelayLifetime::MapOrInstance,
        "source-creature" => RelayLifetime::SourceCreature,
        value => return Err(format!("unknown relay lifetime: {value}")),
    };
    let steps = fields[4]
        .split('~')
        .map(parse_step)
        .collect::<Result<Vec<_>, _>>()?;
    if relay_id == 0 || steps.is_empty() {
        return Err("relay definition needs a nonzero id and at least one step".to_string());
    }
    if steps
        .windows(2)
        .any(|pair| step_key(&pair[0]) > step_key(&pair[1]))
    {
        return Err(format!("relay {relay_id} steps are not in execution order"));
    }
    let expected = encoded_definition_version(fields[0], fields[2], fields[3], fields[4]);
    if version != expected {
        return Err(format!(
            "relay {relay_id} version mismatch: supplied={version} expected={expected}"
        ));
    }
    Ok(RelayDefinition {
        id: 0,
        relay_id,
        version,
        catalogue_version: 0,
        current: true,
        concurrency,
        lifetime,
        steps,
    })
}

fn parse_step(encoded: &str) -> Result<RelayStep, String> {
    let fields = encoded.splitn(5, ',').collect::<Vec<_>>();
    if fields.len() != 5 {
        return Err(format!("relay step needs five fields: {encoded}"));
    }
    Ok(RelayStep {
        offset_ms: parse_u32(fields[0])?,
        command_priority: parse_u32(fields[1])?,
        source_order: parse_u32(fields[2])?,
        participants: parse_participants(fields[3])?,
        instruction: parse_instruction(fields[4])?,
    })
}

fn parse_subject(encoded: &str) -> Result<RelaySubject, String> {
    let fields = encoded.split(':').collect::<Vec<_>>();
    match fields.as_slice() {
        ["source"] => Ok(RelaySubject::Source),
        ["selected"] => Ok(RelaySubject::Selected),
        ["nearby-creature", entry, radius] => Ok(RelaySubject::NearbyCreature(RelayNearby {
            entry: parse_u32(entry)?,
            radius_yd: parse_u32(radius)?,
        })),
        ["nearby-gameobject", entry, radius] => Ok(RelaySubject::NearbyGameObject(RelayNearby {
            entry: parse_u32(entry)?,
            radius_yd: parse_u32(radius)?,
        })),
        _ => Err(format!("unknown relay subject: {encoded}")),
    }
}

fn parse_participants(encoded: &str) -> Result<RelayParticipants, String> {
    let Some((source, target)) = encoded.split_once('>') else {
        return Err(format!("relay participants need source>target: {encoded}"));
    };
    Ok(RelayParticipants {
        source: parse_subject(source)?,
        target: parse_subject(target)?,
    })
}

fn parse_instruction(encoded: &str) -> Result<RelayInstruction, String> {
    let fields = encoded.split(':').collect::<Vec<_>>();
    match fields.as_slice() {
        ["talk", ids] => Ok(RelayInstruction::Talk(RelayTalk {
            broadcast_ids: ids.split('.').map(parse_u32).collect::<Result<_, _>>()?,
        })),
        ["emote", id] => Ok(RelayInstruction::Emote(RelayEmote {
            emote_id: parse_u32(id)?,
        })),
        ["move-to", ms, x, y, z, o] => Ok(RelayInstruction::MoveTo(RelayMoveTo {
            travel_ms: parse_u32(ms)?,
            x: parse_f32(x)?,
            y: parse_f32(y)?,
            z: parse_f32(z)?,
            orientation: parse_f32(o)?,
        })),
        ["spawn-creature", entry, ms, x, y, z, o] => {
            Ok(RelayInstruction::SpawnCreature(RelaySpawnCreature {
                entry: parse_u32(entry)?,
                despawn_ms: parse_u32(ms)?,
                x: parse_f32(x)?,
                y: parse_f32(y)?,
                z: parse_f32(z)?,
                orientation: parse_f32(o)?,
            }))
        }
        ["activate-object"] => Ok(RelayInstruction::ActivateObject),
        ["cast-spell", spell, triggered] => Ok(RelayInstruction::CastSpell(RelayCastSpell {
            spell_id: parse_u32(spell)?,
            triggered: parse_bool(triggered)?,
        })),
        ["despawn-source", delay] => Ok(RelayInstruction::DespawnSource(RelayDelay {
            delay_ms: parse_u32(delay)?,
        })),
        ["set-movement", kind, distance, path] => {
            Ok(RelayInstruction::SetMovement(RelayMovement {
                movement_kind: parse_u8(kind)?,
                distance_yd: parse_u32(distance)?,
                path_id: parse_u32(path)?,
            }))
        }
        ["set-active", enabled] => Ok(RelayInstruction::SetActive(RelayEnabled {
            enabled: parse_bool(enabled)?,
        })),
        ["set-faction", faction, temporary] => Ok(RelayInstruction::SetFaction(RelayFaction {
            faction_template: parse_u32(faction)?,
            temporary: parse_bool(temporary)?,
        })),
        ["set-run", enabled] => Ok(RelayInstruction::SetRun(RelayEnabled {
            enabled: parse_bool(enabled)?,
        })),
        ["attack-start"] => Ok(RelayInstruction::AttackStart),
        ["set-stand-state", state] => Ok(RelayInstruction::SetStandState(RelayStandState {
            stand_state: parse_u8(state)?,
        })),
        ["modify-npc-flags", flags, add] => Ok(RelayInstruction::ModifyNpcFlags(RelayFlagChange {
            flags: parse_u32(flags)?,
            add: parse_bool(add)?,
        })),
        ["terminate-invocation"] => Ok(RelayInstruction::TerminateInvocation),
        ["pause-waypoints", paused] => Ok(RelayInstruction::PauseWaypoints(RelayEnabled {
            enabled: parse_bool(paused)?,
        })),
        ["send-ai-event", kind, radius] => Ok(RelayInstruction::SendAiEvent(RelayAiEvent {
            kind: parse_ai_event(kind)?,
            radius_yd: parse_u32(radius)?,
        })),
        ["set-facing", orientation] => Ok(RelayInstruction::SetFacing(RelayFacing {
            orientation: parse_f32(orientation)?,
        })),
        ["move-dynamic", ms, distance] => Ok(RelayInstruction::MoveDynamic(RelayDynamicMove {
            travel_ms: parse_u32(ms)?,
            distance_yd: parse_u32(distance)?,
        })),
        ["despawn-gameobject", delay] => Ok(RelayInstruction::DespawnGameObject(RelayDelay {
            delay_ms: parse_u32(delay)?,
        })),
        ["set-equipment", main, off, ranged] => {
            Ok(RelayInstruction::SetEquipment(RelayEquipment {
                main_hand: parse_i32(main)?,
                off_hand: parse_i32(off)?,
                ranged: parse_i32(ranged)?,
            }))
        }
        ["update-creature-template", entry, team] => Ok(RelayInstruction::UpdateCreatureTemplate(
            RelayTemplateUpdate {
                entry: parse_u32(entry)?,
                team: parse_u32(team)?,
            },
        )),
        ["modify-unit-flags", flags, add] => {
            Ok(RelayInstruction::ModifyUnitFlags(RelayFlagChange {
                flags: parse_u32(flags)?,
                add: parse_bool(add)?,
            }))
        }
        ["set-gossip-menu", menu] => Ok(RelayInstruction::SetGossipMenu(RelayGossipMenu {
            menu_id: parse_u32(menu)?,
        })),
        ["set-world-state", id, value] => Ok(RelayInstruction::SetWorldState(RelayWorldState {
            world_state_id: parse_u32(id)?,
            value: parse_u32(value)?,
        })),
        ["start-relay", relay] => Ok(RelayInstruction::StartRelay(RelayStart {
            relay_id: parse_u32(relay)?,
        })),
        _ => Err(format!("unknown relay instruction: {encoded}")),
    }
}

fn step_key(step: &RelayStep) -> (u32, u32, u32) {
    (step.offset_ms, step.command_priority, step.source_order)
}

fn encoded_definition_version(relay: &str, concurrency: &str, lifetime: &str, steps: &str) -> u64 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-relay-definition-v1");
    hasher.update(format!("{relay}@{concurrency}@{lifetime}@{steps}").as_bytes());
    u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap())
}

fn encoded_catalogue_version(definitions: &[RelayDefinition]) -> u64 {
    let mut versions = definitions
        .iter()
        .map(|definition| (definition.relay_id, definition.version))
        .collect::<Vec<_>>();
    versions.sort_unstable();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"lyracore-relay-catalogue-v1");
    for (relay_id, version) in versions {
        hasher.update(&relay_id.to_le_bytes());
        hasher.update(&version.to_le_bytes());
    }
    u64::from_le_bytes(hasher.finalize().as_bytes()[..8].try_into().unwrap())
}

fn definition_matches_catalogue(
    definition: &RelayDefinition,
    pinned_catalogue_version: Option<u64>,
) -> bool {
    pinned_catalogue_version
        .map(|version| definition.catalogue_version == version)
        .unwrap_or(definition.current)
}

fn definition_is_retained(
    definition: &RelayDefinition,
    active_catalogues: &std::collections::HashSet<u64>,
) -> bool {
    definition.current || active_catalogues.contains(&definition.catalogue_version)
}

fn step_due_ms(started_at_ms: u64, offset_ms: u32) -> u64 {
    started_at_ms.saturating_add(u64::from(offset_ms))
}

pub(crate) fn start_imported_relay(
    ctx: &ReducerContext,
    relay_id: u32,
    source_guid: u64,
    selected_guid: u64,
    random_state: u64,
) -> Result<u64, String> {
    start_relay(
        ctx,
        relay_id,
        source_guid,
        selected_guid,
        random_state,
        0,
        None,
    )
}

fn start_relay(
    ctx: &ReducerContext,
    relay_id: u32,
    source_guid: u64,
    selected_guid: u64,
    random_state: u64,
    parent_run_id: u64,
    pinned_catalogue_version: Option<u64>,
) -> Result<u64, String> {
    let definition = ctx
        .db
        .game_creature_ai_relay_definition()
        .by_relay()
        .filter(&relay_id)
        .find(|row| definition_matches_catalogue(row, pinned_catalogue_version))
        .ok_or_else(|| format!("relay definition {relay_id} is missing"))?;
    let source = ctx
        .db
        .game_world_entity()
        .guid()
        .find(source_guid)
        .ok_or_else(|| format!("relay source {source_guid} is missing"))?;
    let matching = ctx
        .db
        .game_creature_ai_relay_run()
        .by_source()
        .filter(&source_guid)
        .filter(|run| run.relay_id == relay_id)
        .map(|run| run.id)
        .collect::<Vec<_>>();
    match definition.concurrency {
        RelayConcurrency::Parallel => {}
        RelayConcurrency::IgnoreIfRunning if !matching.is_empty() => return Ok(matching[0]),
        RelayConcurrency::IgnoreIfRunning => {}
        RelayConcurrency::Replace => {
            for run_id in matching {
                cancel_run(ctx, run_id);
            }
        }
    }
    let run = ctx.db.game_creature_ai_relay_run().insert(RelayRun {
        id: 0,
        relay_id,
        definition_id: definition.id,
        definition_version: definition.version,
        catalogue_version: definition.catalogue_version,
        next_step: 0,
        started_at: ctx.timestamp,
        due_at: ctx.timestamp,
        map_id: source.map_id,
        instance_id: source.instance_id,
        source_guid,
        selected_guid,
        saved_random_state: random_state,
        concurrency: definition.concurrency,
        lifetime: definition.lifetime,
        parent_run_id,
    });
    if let Err(error) = advance_run(ctx, run.id) {
        cancel_run(ctx, run.id);
        return Err(error);
    }
    Ok(run.id)
}

fn advance_run(ctx: &ReducerContext, run_id: u64) -> Result<(), String> {
    let runs = ctx.db.game_creature_ai_relay_run();
    let Some(mut run) = runs.id().find(run_id) else {
        return Ok(());
    };
    let definition = ctx
        .db
        .game_creature_ai_relay_definition()
        .id()
        .find(run.definition_id)
        .filter(|row| row.version == run.definition_version)
        .ok_or_else(|| {
            format!(
                "relay run {run_id} lost definition version {}",
                run.definition_version
            )
        })?;
    let now_ms = ctx.timestamp.to_micros_since_unix_epoch().max(0) as u64 / 1000;
    let start_ms = run.started_at.to_micros_since_unix_epoch().max(0) as u64 / 1000;
    while let Some(step) = definition.steps.get(run.next_step as usize) {
        let due_ms = step_due_ms(start_ms, step.offset_ms);
        if due_ms > now_ms {
            run.due_at = Timestamp::from_micros_since_unix_epoch(
                i64::try_from(due_ms.saturating_mul(1000)).unwrap_or(i64::MAX),
            );
            runs.id().update(run);
            schedule_run(ctx, run_id, due_ms);
            return Ok(());
        }
        run.next_step += 1;
        let saved_random_state = run.saved_random_state;
        runs.id().update(run);
        let outcome = match &step.instruction {
            RelayInstruction::TerminateInvocation => {
                cancel_run(ctx, run_id);
                return Ok(());
            }
            RelayInstruction::StartRelay(start) => {
                let fresh = runs
                    .id()
                    .find(run_id)
                    .ok_or_else(|| format!("relay run {run_id} disappeared"))?;
                resolve_participants(&fresh, step.participants).and_then(
                    |(nested_source, nested_target)| {
                        start_relay(
                            ctx,
                            start.relay_id,
                            nested_source,
                            nested_target,
                            saved_random_state,
                            run_id,
                            Some(fresh.catalogue_version),
                        )
                        .map(|_| ())
                    },
                )
            }
            instruction => {
                let fresh = runs
                    .id()
                    .find(run_id)
                    .ok_or_else(|| format!("relay run {run_id} disappeared"))?;
                apply_leaf(ctx, &fresh, step.participants, instruction)
            }
        };
        if let Err(error) = outcome {
            cancel_run(ctx, run_id);
            return Err(format!(
                "relay run {run_id} failed at source step {}: {error}",
                step.source_order
            ));
        }
        let Some(fresh) = runs.id().find(run_id) else {
            return Ok(());
        };
        run = fresh;
    }
    cancel_run(ctx, run_id);
    reap_unused_definitions(ctx);
    Ok(())
}

fn apply_leaf(
    ctx: &ReducerContext,
    run: &RelayRun,
    participants: RelayParticipants,
    instruction: &RelayInstruction,
) -> Result<(), String> {
    let (guid, target_guid) = resolve_participants(run, participants)?;
    match instruction {
        RelayInstruction::Talk(talk) => {
            if talk.broadcast_ids.is_empty() {
                return Err("relay talk has no broadcast text".to_string());
            }
            let id = &talk.broadcast_ids
                [run.saved_random_state as usize % talk.broadcast_ids.len()];
            let line = ctx
                .db
                .game_creature_ai_broadcast_text()
                .id()
                .find(id)
                .ok_or_else(|| format!("relay broadcast text {id} is missing"))?;
            let source = ctx
                .db
                .game_world_entity()
                .guid()
                .find(guid)
                .ok_or_else(|| format!("relay talk subject {guid} is missing"))?;
            crate::chat::apply_send_chat(ctx, source, line.chat_type, line.language_id, line.male_text)
        }
        RelayInstruction::Emote(emote) => {
            let source = ctx
                .db
                .game_world_entity()
                .guid()
                .find(guid)
                .ok_or_else(|| format!("relay emote subject {guid} is missing"))?;
            crate::chat::apply_send_emote(ctx, source, 0, emote.emote_id, target_guid)
        }
        _ => Err(format!(
            "relay {} reached a typed capability without a gameplay authority binding: {instruction:?}",
            run.relay_id
        )),
    }
}

fn resolve_subject(run: &RelayRun, subject: RelaySubject) -> Result<u64, String> {
    match subject {
        RelaySubject::Source => Ok(run.source_guid),
        RelaySubject::Selected => Ok(run.selected_guid),
        RelaySubject::NearbyCreature(_) | RelaySubject::NearbyGameObject(_) => {
            Err("relay nearby-subject resolution is unavailable".to_string())
        }
    }
}

fn resolve_participants(
    run: &RelayRun,
    participants: RelayParticipants,
) -> Result<(u64, u64), String> {
    Ok((
        resolve_subject(run, participants.source)?,
        resolve_subject(run, participants.target)?,
    ))
}

fn schedule_run(ctx: &ReducerContext, run_id: u64, due_ms: u64) {
    let table = ctx.db.game_creature_ai_relay_continuation();
    for row in table.by_run().filter(&run_id).collect::<Vec<_>>() {
        table.scheduled_id().delete(row.scheduled_id);
    }
    let due = Timestamp::from_micros_since_unix_epoch(
        i64::try_from(due_ms.saturating_mul(1000)).unwrap_or(i64::MAX),
    );
    table.insert(RelayContinuation {
        scheduled_id: 0,
        scheduled_at: ScheduleAt::Time(due),
        run_id,
    });
}

#[reducer]
pub fn resume_relay_run(ctx: &ReducerContext, continuation: RelayContinuation) {
    if ctx.sender() != ctx.database_identity() {
        return;
    }
    if let Err(error) = advance_run(ctx, continuation.run_id) {
        cancel_run(ctx, continuation.run_id);
        spacetimedb::log::error!("{error}");
    }
}

fn cancel_run(ctx: &ReducerContext, run_id: u64) {
    let continuations = ctx.db.game_creature_ai_relay_continuation();
    for row in continuations.by_run().filter(&run_id).collect::<Vec<_>>() {
        continuations.scheduled_id().delete(row.scheduled_id);
    }
    ctx.db.game_creature_ai_relay_run().id().delete(run_id);
}

pub(crate) fn cancel_relay_runs_for_instance(ctx: &ReducerContext, instance_id: u64) {
    let run_ids = ctx
        .db
        .game_creature_ai_relay_run()
        .by_instance()
        .filter(&instance_id)
        .map(|run| run.id)
        .collect::<Vec<_>>();
    for run_id in run_ids {
        cancel_run(ctx, run_id);
    }
    reap_unused_definitions(ctx);
}

pub(crate) fn cancel_relay_runs_for_source(ctx: &ReducerContext, source_guid: u64) {
    let run_ids = ctx
        .db
        .game_creature_ai_relay_run()
        .by_source()
        .filter(&source_guid)
        .filter(|run| run.lifetime == RelayLifetime::SourceCreature)
        .map(|run| run.id)
        .collect::<Vec<_>>();
    for run_id in run_ids {
        cancel_run(ctx, run_id);
    }
    reap_unused_definitions(ctx);
}

fn reap_unused_definitions(ctx: &ReducerContext) {
    let active_catalogues = ctx
        .db
        .game_creature_ai_relay_run()
        .iter()
        .map(|run| run.catalogue_version)
        .collect::<std::collections::HashSet<_>>();
    let definitions = ctx.db.game_creature_ai_relay_definition();
    for id in definitions
        .iter()
        .filter(|definition| !definition_is_retained(definition, &active_catalogues))
        .map(|definition| definition.id)
        .collect::<Vec<_>>()
    {
        definitions.id().delete(id);
    }
}

fn parse_ai_event(value: &str) -> Result<super::AiEventKind, String> {
    use super::AiEventKind::*;
    match value {
        "just-died" => Ok(JustDied),
        "critical-health" => Ok(CriticalHealth),
        "lost-health" => Ok(LostHealth),
        "lost-some-health" => Ok(LostSomeHealth),
        "got-full-health" => Ok(GotFullHealth),
        "custom-a" => Ok(CustomA),
        "custom-b" => Ok(CustomB),
        "crowd-controlled" => Ok(CrowdControlled),
        "custom-c" => Ok(CustomC),
        "custom-d" => Ok(CustomD),
        "custom-e" => Ok(CustomE),
        "custom-f" => Ok(CustomF),
        _ => Err(format!("unknown relay AI event: {value}")),
    }
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "0" => Ok(false),
        "1" => Ok(true),
        _ => Err(format!("invalid relay boolean: {value}")),
    }
}
fn parse_u8(value: &str) -> Result<u8, String> {
    value
        .parse()
        .map_err(|_| format!("invalid relay u8: {value}"))
}
fn parse_u32(value: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("invalid relay u32: {value}"))
}
fn parse_u64(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("invalid relay u64: {value}"))
}
fn parse_i32(value: &str) -> Result<i32, String> {
    value
        .parse()
        .map_err(|_| format!("invalid relay i32: {value}"))
}
fn parse_f32(value: &str) -> Result<f32, String> {
    value
        .parse()
        .map_err(|_| format!("invalid relay f32: {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(offset_ms: u32, instruction: RelayInstruction) -> RelayStep {
        RelayStep {
            offset_ms,
            command_priority: 0,
            source_order: 0,
            participants: RelayParticipants {
                source: RelaySubject::Source,
                target: RelaySubject::Selected,
            },
            instruction,
        }
    }

    fn emote(offset_ms: u32) -> RelayStep {
        step(
            offset_ms,
            RelayInstruction::Emote(RelayEmote { emote_id: 1 }),
        )
    }

    fn nested(relay_id: u32) -> RelayStep {
        step(0, RelayInstruction::StartRelay(RelayStart { relay_id }))
    }

    fn definition(relay_id: u32, version: u64, catalogue_version: u64) -> RelayDefinition {
        RelayDefinition {
            id: u64::from(relay_id),
            relay_id,
            version,
            catalogue_version,
            current: true,
            concurrency: RelayConcurrency::Parallel,
            lifetime: RelayLifetime::MapOrInstance,
            steps: vec![emote(0)],
        }
    }

    #[test]
    fn execution_order_uses_priority_then_source_order_at_equal_time() {
        let mut steps = vec![
            RelayStep {
                offset_ms: 20,
                command_priority: 2,
                source_order: 3,
                participants: RelayParticipants {
                    source: RelaySubject::Source,
                    target: RelaySubject::Selected,
                },
                instruction: RelayInstruction::TerminateInvocation,
            },
            RelayStep {
                offset_ms: 20,
                command_priority: 1,
                source_order: 9,
                participants: RelayParticipants {
                    source: RelaySubject::Source,
                    target: RelaySubject::Selected,
                },
                instruction: RelayInstruction::TerminateInvocation,
            },
            RelayStep {
                offset_ms: 20,
                command_priority: 1,
                source_order: 2,
                participants: RelayParticipants {
                    source: RelaySubject::Source,
                    target: RelaySubject::Selected,
                },
                instruction: RelayInstruction::TerminateInvocation,
            },
        ];
        steps.sort_by_key(step_key);
        assert_eq!(
            steps
                .iter()
                .map(|step| (step.command_priority, step.source_order))
                .collect::<Vec<_>>(),
            vec![(1, 2), (1, 9), (2, 3)]
        );
    }

    #[test]
    fn graph_validation_refuses_missing_dependencies_and_cycles() {
        let missing = std::collections::BTreeMap::from([(1, vec![nested(2)])]);
        assert!(validate_definition_graph(&missing)
            .unwrap_err()
            .contains("dependency 2 is missing"));

        let cycle = std::collections::BTreeMap::from([(1, vec![nested(2)]), (2, vec![nested(1)])]);
        assert!(validate_definition_graph(&cycle)
            .unwrap_err()
            .contains("relay cycle: 1 -> 2 -> 1"));
    }

    #[test]
    fn graph_budget_counts_every_nested_invocation() {
        let graph = std::collections::BTreeMap::from([
            (1, vec![nested(2), nested(2), nested(2)]),
            (2, vec![emote(0); 2_048]),
        ]);
        assert!(validate_definition_graph(&graph)
            .unwrap_err()
            .contains("4096-step budget"));

        let scheduled = std::collections::BTreeMap::from([
            (1, vec![nested(2), nested(2)]),
            (2, vec![emote(1); 1_025]),
        ]);
        assert!(validate_definition_graph(&scheduled)
            .unwrap_err()
            .contains("2048 scheduled-work budget"));
    }

    #[test]
    fn loader_refuses_unbound_leaf_capabilities() {
        let graph = std::collections::BTreeMap::from([(
            1,
            vec![step(
                0,
                RelayInstruction::CastSpell(RelayCastSpell {
                    spell_id: 1,
                    triggered: false,
                }),
            )],
        )]);
        assert!(validate_definition_graph(&graph)
            .unwrap_err()
            .contains("no gameplay authority binding"));
    }

    #[test]
    fn delayed_steps_keep_the_invocation_start_as_their_time_base() {
        let due = step_due_ms(10_000, 250);
        assert_eq!(due, 10_250);
        assert_ne!(due, step_due_ms(10_100, 250));
    }

    #[test]
    fn delayed_nested_runs_pin_the_parent_catalogue_across_reload() {
        let old_catalogue = 101;
        let new_catalogue = 202;
        let mut old_child = definition(2, 20, old_catalogue);
        old_child.current = false;
        let new_child = definition(2, 21, new_catalogue);

        assert!(definition_matches_catalogue(
            &old_child,
            Some(old_catalogue)
        ));
        assert!(!definition_matches_catalogue(
            &new_child,
            Some(old_catalogue)
        ));
        assert!(definition_is_retained(
            &old_child,
            &std::collections::HashSet::from([old_catalogue])
        ));
        assert_ne!(
            encoded_catalogue_version(&[definition(1, 10, 0), definition(2, 20, 0)]),
            encoded_catalogue_version(&[definition(1, 10, 0), definition(2, 21, 0)])
        );
    }
}
