use spacetimedb::{reducer, table, ReducerContext, ScheduleAt, Table, Timestamp};

use crate::{
    game_creature_ai_broadcast_text, game_creature_ai_definition, game_creature_template,
    game_faction_template, game_gameobject, game_gameobject_template, game_spell,
    game_world_entity,
};

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
    CreatureGuid(u64),
    GameObjectGuid(u64),
    NearbyCreature(RelayNearby),
    NearbyGameObject(RelayNearby),
    AllNearbyCreatures(RelayNearby),
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
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub struct RelayEmote {
    pub emote_ids: Vec<u32>,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayMovePoint {
    pub movement: RelayForcedMovement,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayFaceOrientation {
    pub orientation: f32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub enum RelayMoveTo {
    Point(RelayMovePoint),
    FaceOrientation(RelayFaceOrientation),
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelaySpawnCreature {
    pub entry: u32,
    pub despawn_ms: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub orientation: f32,
    pub active: bool,
    pub run_by_default: bool,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayActivateObject {
    pub animation_id: u32,
    pub custom_animation_id: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayForcedMovement {
    Inherit,
    Walk,
    Run,
}
#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub struct RelayCastSpell {
    pub spell_ids: Vec<u32>,
    pub triggered: bool,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayDelay {
    pub delay_ms: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayRandomMovement {
    pub radius_yd: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayPatrolMovement {
    pub path_id: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub enum RelayMovement {
    Stationary,
    RandomAroundCurrent(RelayRandomMovement),
    Patrol(RelayPatrolMovement),
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelaySetMovement {
    pub idle: RelayMovement,
    pub forced: RelayForcedMovement,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayEnabled {
    pub enabled: bool,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayFaction {
    pub faction_template: u32,
    pub restoration: RelayFactionRestoration,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayFactionRestoration {
    Permanent,
    OnCombatStopOrRespawn,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayStandState {
    pub stand_state: u8,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayFlagChange {
    pub flags: u32,
    pub operation: RelayFlagOperation,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayFlagOperation {
    Add,
    Remove,
    Toggle,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayAiEvent {
    pub kind: super::AiEventKind,
    pub radius_yd: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub enum RelayFacing {
    Target,
    Reset,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayDynamicMove {
    pub minimum_distance_yd: u32,
    pub maximum_distance_yd: u32,
    pub fixed_distance_yd: u32,
    pub movement: RelayForcedMovement,
    pub movement_flags: u32,
}
#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, PartialEq)]
pub struct RelayEquipment {
    pub reset_default: bool,
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

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayTerminateWhen {
    Present,
    Missing,
}

#[derive(spacetimedb::SpacetimeType, Clone, Copy, Debug, Eq, PartialEq)]
pub struct RelayTerminate {
    pub subject: RelaySubject,
    pub when: RelayTerminateWhen,
}

#[derive(spacetimedb::SpacetimeType, Clone, Debug, PartialEq)]
pub enum RelayInstruction {
    Talk(RelayTalk),
    Emote(RelayEmote),
    MoveTo(RelayMoveTo),
    SpawnCreature(RelaySpawnCreature),
    ActivateObject(RelayActivateObject),
    CastSpell(RelayCastSpell),
    DespawnSource(RelayDelay),
    SetMovement(RelaySetMovement),
    SetActive(RelayEnabled),
    SetFaction(RelayFaction),
    SetRun(RelayEnabled),
    AttackStart,
    SetStandState(RelayStandState),
    ModifyNpcFlags(RelayFlagChange),
    TerminateInvocation(RelayTerminate),
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
    index(accessor = by_source, btree(columns = [source_guid])),
    index(accessor = by_parent, btree(columns = [parent_run_id]))
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
    if definitions.is_empty() {
        return Err("relay catalogue needs at least one definition".to_string());
    }
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
    for definition in definitions {
        for step in &definition.steps {
            match &step.instruction {
                RelayInstruction::Talk(talk) => {
                    for broadcast_id in &talk.broadcast_ids {
                        if ctx
                            .db
                            .game_creature_ai_broadcast_text()
                            .id()
                            .find(broadcast_id)
                            .is_none()
                        {
                            return Err(missing_dependency(
                                definition,
                                step,
                                "broadcast_text",
                                *broadcast_id,
                            ));
                        }
                    }
                }
                RelayInstruction::SpawnCreature(spawn)
                    if ctx
                        .db
                        .game_creature_template()
                        .entry()
                        .find(spawn.entry)
                        .is_none() =>
                {
                    return Err(missing_dependency(
                        definition,
                        step,
                        "creature_template",
                        spawn.entry,
                    ));
                }
                RelayInstruction::CastSpell(cast) => {
                    for spell_id in &cast.spell_ids {
                        if ctx.db.game_spell().spell_id().find(spell_id).is_none() {
                            return Err(missing_dependency(definition, step, "spell", *spell_id));
                        }
                    }
                }
                RelayInstruction::SetFaction(faction)
                    if ctx
                        .db
                        .game_faction_template()
                        .id()
                        .find(faction.faction_template)
                        .is_none() =>
                {
                    return Err(missing_dependency(
                        definition,
                        step,
                        "faction_template",
                        faction.faction_template,
                    ));
                }
                _ => {}
            }
            validate_subject_dependency(ctx, definition, step, step.participants.source)?;
            validate_subject_dependency(ctx, definition, step, step.participants.target)?;
            if let RelayInstruction::TerminateInvocation(terminate) = step.instruction {
                validate_subject_dependency(ctx, definition, step, terminate.subject)?;
            }
        }
    }
    Ok(())
}

fn missing_dependency(
    definition: &RelayDefinition,
    step: &RelayStep,
    kind: &str,
    id: u32,
) -> String {
    format!(
        "relay dependency path {} -> row:{} -> {kind}:{id} is missing",
        definition.relay_id, step.source_order
    )
}

fn validate_subject_dependency(
    ctx: &ReducerContext,
    definition: &RelayDefinition,
    step: &RelayStep,
    subject: RelaySubject,
) -> Result<(), String> {
    let (kind, entry, exists) = match subject {
        RelaySubject::NearbyCreature(nearby) | RelaySubject::AllNearbyCreatures(nearby) => (
            "creature_template",
            nearby.entry,
            ctx.db
                .game_creature_template()
                .entry()
                .find(nearby.entry)
                .is_some(),
        ),
        RelaySubject::NearbyGameObject(nearby) => (
            "gameobject_template",
            nearby.entry,
            ctx.db
                .game_gameobject_template()
                .entry()
                .find(nearby.entry)
                .is_some(),
        ),
        _ => return Ok(()),
    };
    exists
        .then_some(())
        .ok_or_else(|| missing_dependency(definition, step, kind, entry))
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
    if matches!(
        step.participants.target,
        RelaySubject::AllNearbyCreatures(_)
    ) {
        return Err(format!(
            "relay {relay_id} step {} cannot use a participant set as its target",
            step.source_order
        ));
    }
    match &step.instruction {
        RelayInstruction::Talk(talk)
            if talk.broadcast_ids.is_empty() || talk.broadcast_ids.contains(&0) =>
        {
            Err(format!(
                "relay {relay_id} step {} has no broadcast text",
                step.source_order
            ))
        }
        RelayInstruction::Emote(emote)
            if emote.emote_ids.is_empty() || emote.emote_ids.contains(&0) =>
        {
            Err(format!(
                "relay {relay_id} step {} has no emote",
                step.source_order
            ))
        }
        RelayInstruction::CastSpell(cast)
            if cast.spell_ids.is_empty() || cast.spell_ids.contains(&0) =>
        {
            Err(format!(
                "relay {relay_id} step {} has no spell",
                step.source_order
            ))
        }
        RelayInstruction::DespawnSource(delay) | RelayInstruction::DespawnGameObject(delay)
            if delay.delay_ms != 0 =>
        {
            Err(format!(
                "relay {relay_id} step {} needs an unavailable delayed-despawn authority",
                step.source_order
            ))
        }
        RelayInstruction::ModifyNpcFlags(change)
            if change.flags == 0 || change.flags & !0x3 != 0 =>
        {
            Err(format!(
                "relay {relay_id} step {} has unsupported named NPC flags {:#x}",
                step.source_order, change.flags
            ))
        }
        RelayInstruction::ModifyUnitFlags(change)
            if change.flags == 0 || change.flags & !0x300 != 0 =>
        {
            Err(format!(
                "relay {relay_id} step {} has unsupported named unit flags {:#x}",
                step.source_order, change.flags
            ))
        }
        RelayInstruction::SetEquipment(equipment)
            if equipment.reset_default
                || equipment.main_hand < 0
                || equipment.off_hand < 0
                || equipment.ranged < 0 =>
        {
            Err(format!(
                "relay {relay_id} step {} needs an unavailable equipment-reset authority",
                step.source_order
            ))
        }
        RelayInstruction::MoveDynamic(movement) if movement.movement_flags != 0 => Err(format!(
            "relay {relay_id} step {} needs unavailable dynamic movement flags {:#x}",
            step.source_order, movement.movement_flags
        )),
        instruction @ (RelayInstruction::ActivateObject(_)
        | RelayInstruction::SetActive(_)
        | RelayInstruction::UpdateCreatureTemplate(_)
        | RelayInstruction::SetGossipMenu(_)
        | RelayInstruction::SetWorldState(_)) => Err(format!(
            "relay {relay_id} step {} has no gameplay authority binding for {instruction:?}",
            step.source_order
        )),
        _ => Ok(()),
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
        ["creature-guid", guid] => Ok(RelaySubject::CreatureGuid(parse_u64(guid)?)),
        ["gameobject-guid", guid] => Ok(RelaySubject::GameObjectGuid(parse_u64(guid)?)),
        ["nearby-creature", entry, radius] => Ok(RelaySubject::NearbyCreature(RelayNearby {
            entry: parse_u32(entry)?,
            radius_yd: parse_u32(radius)?,
        })),
        ["nearby-gameobject", entry, radius] => Ok(RelaySubject::NearbyGameObject(RelayNearby {
            entry: parse_u32(entry)?,
            radius_yd: parse_u32(radius)?,
        })),
        ["all-nearby-creatures", entry, radius] => {
            Ok(RelaySubject::AllNearbyCreatures(RelayNearby {
                entry: parse_u32(entry)?,
                radius_yd: parse_u32(radius)?,
            }))
        }
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
    if let Some(subject) = encoded.strip_prefix("terminate-if-present:") {
        return Ok(RelayInstruction::TerminateInvocation(RelayTerminate {
            subject: parse_subject(subject)?,
            when: RelayTerminateWhen::Present,
        }));
    }
    if let Some(subject) = encoded.strip_prefix("terminate-if-missing:") {
        return Ok(RelayInstruction::TerminateInvocation(RelayTerminate {
            subject: parse_subject(subject)?,
            when: RelayTerminateWhen::Missing,
        }));
    }
    let fields = encoded.split(':').collect::<Vec<_>>();
    match fields.as_slice() {
        ["talk", ids] => Ok(RelayInstruction::Talk(RelayTalk {
            broadcast_ids: ids.split('.').map(parse_u32).collect::<Result<_, _>>()?,
        })),
        ["emote", ids] => Ok(RelayInstruction::Emote(RelayEmote {
            emote_ids: ids.split('.').map(parse_u32).collect::<Result<_, _>>()?,
        })),
        ["move-point", movement, x, y, z, o] => Ok(RelayInstruction::MoveTo(RelayMoveTo::Point(
            RelayMovePoint {
                movement: parse_forced_movement(movement)?,
                x: parse_f32(x)?,
                y: parse_f32(y)?,
                z: parse_f32(z)?,
                orientation: parse_f32(o)?,
            },
        ))),
        ["face-orientation", orientation] => Ok(RelayInstruction::MoveTo(
            RelayMoveTo::FaceOrientation(RelayFaceOrientation {
                orientation: parse_f32(orientation)?,
            }),
        )),
        ["spawn-creature", entry, ms, active, run, x, y, z, o] => {
            Ok(RelayInstruction::SpawnCreature(RelaySpawnCreature {
                entry: parse_u32(entry)?,
                despawn_ms: parse_u32(ms)?,
                active: parse_bool(active)?,
                run_by_default: parse_bool(run)?,
                x: parse_f32(x)?,
                y: parse_f32(y)?,
                z: parse_f32(z)?,
                orientation: parse_f32(o)?,
            }))
        }
        ["activate-object", animation, custom] => {
            Ok(RelayInstruction::ActivateObject(RelayActivateObject {
                animation_id: parse_u32(animation)?,
                custom_animation_id: parse_u32(custom)?,
            }))
        }
        ["cast-spell", spells, start] => Ok(RelayInstruction::CastSpell(RelayCastSpell {
            spell_ids: spells.split('.').map(parse_u32).collect::<Result<_, _>>()?,
            triggered: match *start {
                "direct" => false,
                "triggered" => true,
                value => return Err(format!("unknown relay spell start mode: {value}")),
            },
        })),
        ["despawn-source", delay] => Ok(RelayInstruction::DespawnSource(RelayDelay {
            delay_ms: parse_u32(delay)?,
        })),
        ["set-movement", "stationary", forced] => {
            Ok(RelayInstruction::SetMovement(RelaySetMovement {
                idle: RelayMovement::Stationary,
                forced: parse_forced_movement(forced)?,
            }))
        }
        ["set-movement", "random-current", radius, forced] => {
            Ok(RelayInstruction::SetMovement(RelaySetMovement {
                idle: RelayMovement::RandomAroundCurrent(RelayRandomMovement {
                    radius_yd: parse_u32(radius)?,
                }),
                forced: parse_forced_movement(forced)?,
            }))
        }
        ["set-movement", "patrol", path, forced] => {
            Ok(RelayInstruction::SetMovement(RelaySetMovement {
                idle: RelayMovement::Patrol(RelayPatrolMovement {
                    path_id: parse_u32(path)?,
                }),
                forced: parse_forced_movement(forced)?,
            }))
        }
        ["set-active", enabled] => Ok(RelayInstruction::SetActive(RelayEnabled {
            enabled: parse_bool(enabled)?,
        })),
        ["set-faction", faction, restoration] => Ok(RelayInstruction::SetFaction(RelayFaction {
            faction_template: parse_u32(faction)?,
            restoration: match *restoration {
                "permanent" => RelayFactionRestoration::Permanent,
                "combat-stop-or-respawn" => RelayFactionRestoration::OnCombatStopOrRespawn,
                value => return Err(format!("unknown relay faction restoration: {value}")),
            },
        })),
        ["set-run", enabled] => Ok(RelayInstruction::SetRun(RelayEnabled {
            enabled: parse_bool(enabled)?,
        })),
        ["attack-start"] => Ok(RelayInstruction::AttackStart),
        ["set-stand-state", state] => Ok(RelayInstruction::SetStandState(RelayStandState {
            stand_state: parse_u8(state)?,
        })),
        ["modify-npc-flags", flags, operation] => {
            Ok(RelayInstruction::ModifyNpcFlags(RelayFlagChange {
                flags: parse_u32(flags)?,
                operation: parse_flag_operation(operation)?,
            }))
        }
        ["pause-waypoints", paused] => Ok(RelayInstruction::PauseWaypoints(RelayEnabled {
            enabled: parse_bool(paused)?,
        })),
        ["send-ai-event", kind, radius] => Ok(RelayInstruction::SendAiEvent(RelayAiEvent {
            kind: parse_ai_event(kind)?,
            radius_yd: parse_u32(radius)?,
        })),
        ["set-facing", "target"] => Ok(RelayInstruction::SetFacing(RelayFacing::Target)),
        ["set-facing", "reset"] => Ok(RelayInstruction::SetFacing(RelayFacing::Reset)),
        ["move-dynamic", minimum, maximum, fixed, movement, flags] => {
            Ok(RelayInstruction::MoveDynamic(RelayDynamicMove {
                minimum_distance_yd: parse_u32(minimum)?,
                maximum_distance_yd: parse_u32(maximum)?,
                fixed_distance_yd: parse_u32(fixed)?,
                movement: parse_forced_movement(movement)?,
                movement_flags: parse_u32(flags)?,
            }))
        }
        ["despawn-gameobject", delay] => Ok(RelayInstruction::DespawnGameObject(RelayDelay {
            delay_ms: parse_u32(delay)?,
        })),
        ["set-equipment", reset, main, off, ranged] => {
            Ok(RelayInstruction::SetEquipment(RelayEquipment {
                reset_default: parse_bool(reset)?,
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
        ["modify-unit-flags", flags, operation] => {
            Ok(RelayInstruction::ModifyUnitFlags(RelayFlagChange {
                flags: parse_u32(flags)?,
                operation: parse_flag_operation(operation)?,
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

#[derive(Debug, Eq, PartialEq)]
enum RelayStartDisposition {
    Start,
    Existing(u64),
    Replace(Vec<u64>),
}

fn start_disposition(
    concurrency: RelayConcurrency,
    mut matching_run_ids: Vec<u64>,
) -> RelayStartDisposition {
    matching_run_ids.sort_unstable();
    match (concurrency, matching_run_ids.as_slice()) {
        (RelayConcurrency::IgnoreIfRunning, [first, ..]) => RelayStartDisposition::Existing(*first),
        (RelayConcurrency::Replace, _) => RelayStartDisposition::Replace(matching_run_ids),
        _ => RelayStartDisposition::Start,
    }
}

fn termination_matches(when: RelayTerminateWhen, subject_present: bool) -> bool {
    match when {
        RelayTerminateWhen::Present => subject_present,
        RelayTerminateWhen::Missing => !subject_present,
    }
}

fn source_lifetime_ends(lifetime: RelayLifetime) -> bool {
    lifetime == RelayLifetime::SourceCreature
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
    catalogue_version: u64,
) -> Result<u64, String> {
    start_relay(
        ctx,
        relay_id,
        source_guid,
        selected_guid,
        random_state,
        0,
        Some(catalogue_version),
    )
}

pub(crate) fn current_catalogue_version(ctx: &ReducerContext) -> Option<u64> {
    let mut versions = ctx
        .db
        .game_creature_ai_relay_definition()
        .iter()
        .filter(|definition| definition.current)
        .map(|definition| definition.catalogue_version);
    let version = versions.next()?;
    versions
        .all(|candidate| candidate == version)
        .then_some(version)
}

pub(crate) fn catalogue_contains(
    ctx: &ReducerContext,
    catalogue_version: u64,
    relay_id: u32,
) -> bool {
    ctx.db
        .game_creature_ai_relay_definition()
        .by_relay()
        .filter(&relay_id)
        .any(|definition| definition.catalogue_version == catalogue_version)
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
    match start_disposition(definition.concurrency, matching) {
        RelayStartDisposition::Start => {}
        RelayStartDisposition::Existing(run_id) => return Ok(run_id),
        RelayStartDisposition::Replace(run_ids) => {
            for run_id in run_ids {
                cancel_run_tree(ctx, run_id);
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
        cancel_run_tree(ctx, run.id);
        reap_unused_definitions(ctx);
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
        runs.id().update(run);
        let outcome = (|| -> Result<(), String> {
            match &step.instruction {
                RelayInstruction::TerminateInvocation(terminate) => {
                    let fresh = runs
                        .id()
                        .find(run_id)
                        .ok_or_else(|| format!("relay run {run_id} disappeared"))?;
                    let present = subject_present(ctx, &fresh, terminate.subject);
                    let terminate_now = termination_matches(terminate.when, present);
                    if terminate_now {
                        remove_run(ctx, run_id);
                        return Ok(());
                    }
                    Ok(())
                }
                RelayInstruction::StartRelay(start) => {
                    let mut fresh = runs
                        .id()
                        .find(run_id)
                        .ok_or_else(|| format!("relay run {run_id} disappeared"))?;
                    let participants = resolve_participants(ctx, &fresh, step.participants)?;
                    let [(nested_source, nested_target)] = participants.as_slice() else {
                        return Err(format!(
                            "relay run {run_id} nested step {} needs one source and one target",
                            step.source_order
                        ));
                    };
                    let (child_state, parent_state) = next_random(fresh.saved_random_state);
                    fresh.saved_random_state = parent_state;
                    let catalogue_version = fresh.catalogue_version;
                    runs.id().update(fresh);
                    start_relay(
                        ctx,
                        start.relay_id,
                        *nested_source,
                        *nested_target,
                        child_state,
                        run_id,
                        Some(catalogue_version),
                    )
                    .map(|_| ())
                }
                instruction => {
                    let mut fresh = runs
                        .id()
                        .find(run_id)
                        .ok_or_else(|| format!("relay run {run_id} disappeared"))?;
                    let outcome = apply_leaf(ctx, &mut fresh, step.participants, instruction);
                    if runs.id().find(run_id).is_some() {
                        runs.id().update(fresh);
                    }
                    outcome
                }
            }
        })();
        if let Err(error) = outcome {
            cancel_run_tree(ctx, run_id);
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
    remove_run(ctx, run_id);
    reap_unused_definitions(ctx);
    Ok(())
}

fn apply_leaf(
    ctx: &ReducerContext,
    run: &mut RelayRun,
    participants: RelayParticipants,
    instruction: &RelayInstruction,
) -> Result<(), String> {
    let pairs = resolve_participants(ctx, run, participants)?;
    for (guid, target_guid) in pairs {
        apply_leaf_for_pair(ctx, run, guid, target_guid, instruction)?;
    }
    Ok(())
}

fn apply_leaf_for_pair(
    ctx: &ReducerContext,
    run: &mut RelayRun,
    guid: u64,
    target_guid: u64,
    instruction: &RelayInstruction,
) -> Result<(), String> {
    match instruction {
        RelayInstruction::Talk(talk) => {
            if talk.broadcast_ids.is_empty() {
                return Err("relay talk has no broadcast text".to_string());
            }
            let id = choose(&talk.broadcast_ids, &mut run.saved_random_state);
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
            let emote_id = choose(&emote.emote_ids, &mut run.saved_random_state);
            let source = ctx
                .db
                .game_world_entity()
                .guid()
                .find(guid)
                .ok_or_else(|| format!("relay emote subject {guid} is missing"))?;
            crate::chat::apply_send_emote(ctx, source, 0, emote_id, target_guid)
        }
        RelayInstruction::MoveTo(movement) => match *movement {
            RelayMoveTo::Point(RelayMovePoint {
                movement,
                x,
                y,
                z,
                orientation,
            }) => {
                crate::encounter::move_to_point(
                    ctx,
                    guid,
                    x,
                    y,
                    z,
                    relay_runs(ctx, guid, movement),
                )?;
                super::movement::apply_relay_orientation(ctx, guid, orientation)
            }
            RelayMoveTo::FaceOrientation(RelayFaceOrientation { orientation }) => {
                super::movement::apply_relay_orientation(ctx, guid, orientation)
            }
        },
        RelayInstruction::SpawnCreature(spawn) => super::mobility::place_relay_summon(
            ctx,
            guid,
            spawn.entry,
            super::SummonLocation {
                x: spawn.x,
                y: spawn.y,
                z: spawn.z,
                orientation: spawn.orientation,
                lifetime_ms: spawn.despawn_ms,
            },
            spawn.active,
            spawn.run_by_default,
        )
        .map(|_| ()),
        RelayInstruction::CastSpell(cast) => {
            let spell_id = choose(&cast.spell_ids, &mut run.saved_random_state);
            let caster = ctx
                .db
                .game_world_entity()
                .guid()
                .find(guid)
                .ok_or_else(|| format!("relay spell caster {guid} is missing"))?;
            if caster.is_player() {
                return Err("relay spell caster must be a creature".to_string());
            }
            let target = if target_guid == 0 {
                crate::spell::CreatureSpellTarget::None
            } else {
                crate::spell::CreatureSpellTarget::Unit(target_guid)
            };
            crate::spell::start_creature_spell(
                ctx,
                crate::spell::CreatureSpellStart {
                    caster_guid: guid,
                    caster_level: caster.level as u8,
                    spell_id,
                    mode: if cast.triggered {
                        crate::spell::CreatureSpellStartMode::Triggered
                    } else {
                        crate::spell::CreatureSpellStartMode::Direct
                    },
                    target,
                    interrupt_previous: cast.triggered,
                    admission: if caster.dead {
                        crate::spell::CreatureSpellCasterAdmission::DeadCreatureCallback
                    } else {
                        crate::spell::CreatureSpellCasterAdmission::Living
                    },
                },
            )
            .map(|_| ())
        }
        RelayInstruction::DespawnSource(_) => {
            if ctx.db.game_world_entity().guid().find(guid).is_none() {
                return Err(format!("relay despawn subject {guid} is missing"));
            }
            crate::creatures::despawn_creature_entity(ctx, guid);
            Ok(())
        }
        RelayInstruction::SetMovement(movement) => {
            super::movement::apply_relay_idle(ctx, guid, movement.idle, movement.forced)
        }
        RelayInstruction::SetFaction(faction) => crate::creatures::presentation::apply_relay_faction(
            ctx,
            guid,
            faction.faction_template,
            faction.restoration == RelayFactionRestoration::OnCombatStopOrRespawn,
        ),
        RelayInstruction::SetRun(enabled) => super::movement::apply_relay_walking(
            ctx,
            guid,
            if enabled.enabled {
                RelayForcedMovement::Run
            } else {
                RelayForcedMovement::Walk
            },
        ),
        RelayInstruction::AttackStart => crate::combat::apply_start_attack(ctx, guid, target_guid),
        RelayInstruction::SetStandState(stand) => {
            crate::creatures::presentation::apply_relay_stand_state(ctx, guid, stand.stand_state)
        }
        RelayInstruction::ModifyNpcFlags(change) => {
            crate::creatures::presentation::apply_relay_npc_flags(
                ctx,
                guid,
                change.flags,
                change.operation,
            )
        }
        RelayInstruction::PauseWaypoints(paused) => super::movement::apply_relay_patrol_pause(
            ctx,
            guid,
            paused.enabled,
        ),
        RelayInstruction::SendAiEvent(event) => super::edges::send_relay_ai_event(
            ctx,
            guid,
            target_guid,
            event.kind,
            event.radius_yd,
        ),
        RelayInstruction::SetFacing(facing) => super::movement::apply_relay_facing(
            ctx,
            guid,
            match facing {
                RelayFacing::Target => Some(target_guid),
                RelayFacing::Reset => None,
            },
        ),
        RelayInstruction::MoveDynamic(dynamic) => {
            let (target_x, target_y, target_z) = participant_position(ctx, target_guid)?;
            let source = ctx
                .db
                .game_world_entity()
                .guid()
                .find(guid)
                .ok_or_else(|| format!("relay dynamic mover {guid} is missing"))?;
            let (x, y, z) = dynamic_destination(
                (source.x, source.y, source.z),
                (target_x, target_y, target_z),
                dynamic.minimum_distance_yd,
                dynamic.maximum_distance_yd,
                dynamic.fixed_distance_yd,
                &mut run.saved_random_state,
            );
            crate::encounter::move_to_point(
                ctx,
                guid,
                x,
                y,
                z,
                relay_runs(ctx, guid, dynamic.movement),
            )?;
            Ok(())
        }
        RelayInstruction::DespawnGameObject(_) => {
            crate::gameobject::despawn_from_relay(ctx, target_guid)
        }
        RelayInstruction::SetEquipment(equipment) => crate::encounter::equip_swap(
            ctx,
            guid,
            equipment.main_hand as u32,
            equipment.off_hand as u32,
            equipment.ranged as u32,
        ),
        RelayInstruction::ModifyUnitFlags(change) => {
            crate::creatures::presentation::apply_relay_unit_flags(
                ctx,
                guid,
                change.flags,
                change.operation,
            )
        }
        RelayInstruction::ActivateObject(_)
        | RelayInstruction::SetActive(_)
        | RelayInstruction::UpdateCreatureTemplate(_)
        | RelayInstruction::SetGossipMenu(_)
        | RelayInstruction::SetWorldState(_) => Err(format!(
            "relay {} reached a typed capability without a gameplay authority binding: {instruction:?}",
            run.relay_id
        )),
        RelayInstruction::TerminateInvocation(_) | RelayInstruction::StartRelay(_) => {
            Err("relay control instruction reached leaf dispatch".to_string())
        }
    }
}

fn choose<T: Copy>(choices: &[T], state: &mut u64) -> T {
    if choices.len() == 1 {
        return choices[0];
    }
    let (value, next) = next_random(*state);
    *state = next;
    choices[value as usize % choices.len()]
}

fn next_random(state: u64) -> (u64, u64) {
    let next = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut value = next;
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (value ^ (value >> 31), next)
}

fn relay_runs(ctx: &ReducerContext, guid: u64, movement: RelayForcedMovement) -> bool {
    match movement {
        RelayForcedMovement::Walk => false,
        RelayForcedMovement::Run => true,
        RelayForcedMovement::Inherit => super::movement::relay_runs_by_default(ctx, guid),
    }
}

fn resolve_subject(
    ctx: &ReducerContext,
    run: &RelayRun,
    subject: RelaySubject,
) -> Result<Vec<u64>, String> {
    match subject {
        RelaySubject::Source => {
            live_unit_in_run(ctx, run, run.source_guid).map(|_| vec![run.source_guid])
        }
        RelaySubject::Selected => {
            live_participant_in_run(ctx, run, run.selected_guid).map(|_| vec![run.selected_guid])
        }
        RelaySubject::CreatureGuid(guid) => live_unit_in_run(ctx, run, guid).map(|_| vec![guid]),
        RelaySubject::GameObjectGuid(guid) => {
            live_gameobject_in_run(ctx, run, guid).map(|_| vec![guid])
        }
        RelaySubject::NearbyCreature(nearby) => nearby_creatures(ctx, run, nearby)
            .into_iter()
            .next()
            .map(|guid| vec![guid])
            .ok_or_else(|| format!("relay nearby creature {} is missing", nearby.entry)),
        RelaySubject::NearbyGameObject(nearby) => nearby_gameobjects(ctx, run, nearby)
            .into_iter()
            .next()
            .map(|guid| vec![guid])
            .ok_or_else(|| format!("relay nearby gameobject {} is missing", nearby.entry)),
        RelaySubject::AllNearbyCreatures(nearby) => {
            let guids = nearby_creatures(ctx, run, nearby);
            (!guids.is_empty())
                .then_some(guids)
                .ok_or_else(|| format!("relay nearby creatures {} are missing", nearby.entry))
        }
    }
}

fn resolve_participants(
    ctx: &ReducerContext,
    run: &RelayRun,
    participants: RelayParticipants,
) -> Result<Vec<(u64, u64)>, String> {
    let sources = resolve_subject(ctx, run, participants.source)?;
    let targets = resolve_subject(ctx, run, participants.target)?;
    if targets.len() != 1 {
        return Err("relay target resolved to more than one participant".to_string());
    }
    Ok(sources
        .into_iter()
        .map(|source| (source, targets[0]))
        .collect())
}

fn subject_present(ctx: &ReducerContext, run: &RelayRun, subject: RelaySubject) -> bool {
    resolve_subject(ctx, run, subject).is_ok()
}

fn live_unit_in_run(
    ctx: &ReducerContext,
    run: &RelayRun,
    guid: u64,
) -> Result<crate::WorldEntity, String> {
    ctx.db
        .game_world_entity()
        .guid()
        .find(guid)
        .filter(|entity| entity.map_id == run.map_id && entity.instance_id == run.instance_id)
        .ok_or_else(|| format!("relay unit participant {guid} is missing"))
}

fn live_gameobject_in_run(
    ctx: &ReducerContext,
    run: &RelayRun,
    guid: u64,
) -> Result<crate::gameobject::GameObject, String> {
    ctx.db
        .game_gameobject()
        .guid()
        .find(guid)
        .filter(|object| object.map_id == run.map_id && object.instance_id == run.instance_id)
        .ok_or_else(|| format!("relay gameobject participant {guid} is missing"))
}

fn live_participant_in_run(ctx: &ReducerContext, run: &RelayRun, guid: u64) -> Result<(), String> {
    if guid == 0 {
        return Err("relay selected participant is missing".to_string());
    }
    if live_unit_in_run(ctx, run, guid).is_ok() || live_gameobject_in_run(ctx, run, guid).is_ok() {
        Ok(())
    } else {
        Err(format!("relay participant {guid} is missing"))
    }
}

fn search_center(ctx: &ReducerContext, run: &RelayRun) -> Result<crate::WorldEntity, String> {
    live_unit_in_run(ctx, run, run.source_guid)
}

fn nearby_creatures(ctx: &ReducerContext, run: &RelayRun, nearby: RelayNearby) -> Vec<u64> {
    let Ok(center) = search_center(ctx, run) else {
        return Vec::new();
    };
    let radius = nearby.radius_yd as f32;
    let radius_sq = radius * radius;
    let mut candidates =
        crate::helpers::entities_near(ctx, run.map_id, run.instance_id, center.x, center.y, radius)
            .into_iter()
            .filter(|candidate| {
                candidate.entry == nearby.entry
                    && !candidate.dead
                    && distance_sq(
                        (center.x, center.y, center.z),
                        (candidate.x, candidate.y, candidate.z),
                    ) <= radius_sq
            })
            .map(|candidate| candidate.guid)
            .collect::<Vec<_>>();
    candidates.sort_unstable();
    candidates.dedup();
    candidates
}

fn nearby_gameobjects(ctx: &ReducerContext, run: &RelayRun, nearby: RelayNearby) -> Vec<u64> {
    let Ok(center) = search_center(ctx, run) else {
        return Vec::new();
    };
    let radius = nearby.radius_yd as f32;
    let radius_sq = radius * radius;
    let (gx0, gx1, gy0, gy1) =
        lyracore_shared::spatial::covering_cell_box(center.x, center.y, radius);
    let objects = ctx.db.game_gameobject();
    let mut candidates = Vec::new();
    for gx in gx0..=gx1 {
        for gy in gy0..=gy1 {
            let cell = lyracore_shared::spatial::grid_cell_id(gx, gy);
            candidates.extend(
                objects
                    .by_cell()
                    .filter((run.map_id, run.instance_id, cell))
                    .filter(|object| {
                        object.template_entry == nearby.entry
                            && distance_sq(
                                (center.x, center.y, center.z),
                                (object.x, object.y, object.z),
                            ) <= radius_sq
                    })
                    .map(|object| object.guid),
            );
        }
    }
    candidates.sort_unstable();
    candidates.dedup();
    candidates
}

fn distance_sq(first: (f32, f32, f32), second: (f32, f32, f32)) -> f32 {
    let (dx, dy, dz) = (first.0 - second.0, first.1 - second.1, first.2 - second.2);
    dx * dx + dy * dy + dz * dz
}

fn participant_position(ctx: &ReducerContext, guid: u64) -> Result<(f32, f32, f32), String> {
    if let Some(entity) = ctx.db.game_world_entity().guid().find(guid) {
        return Ok((entity.x, entity.y, entity.z));
    }
    ctx.db
        .game_gameobject()
        .guid()
        .find(guid)
        .map(|object| (object.x, object.y, object.z))
        .ok_or_else(|| format!("relay participant {guid} has no position"))
}

fn dynamic_destination(
    source: (f32, f32, f32),
    target: (f32, f32, f32),
    minimum_distance_yd: u32,
    maximum_distance_yd: u32,
    fixed_distance_yd: u32,
    state: &mut u64,
) -> (f32, f32, f32) {
    if maximum_distance_yd == 0 {
        if fixed_distance_yd == 0 {
            return target;
        }
        let angle = (target.1 - source.1).atan2(target.0 - source.0);
        return (
            target.0 - angle.cos() * fixed_distance_yd as f32,
            target.1 - angle.sin() * fixed_distance_yd as f32,
            target.2,
        );
    }
    let (roll, next) = next_random(*state);
    *state = next;
    let span = maximum_distance_yd.saturating_sub(minimum_distance_yd);
    let distance = minimum_distance_yd + (roll as u32 % span.saturating_add(1));
    let angle = (target.1 - source.1).atan2(target.0 - source.0);
    (
        target.0 - angle.cos() * distance as f32,
        target.1 - angle.sin() * distance as f32,
        target.2,
    )
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
        cancel_run_tree(ctx, continuation.run_id);
        reap_unused_definitions(ctx);
        spacetimedb::log::error!("{error}");
    }
}

fn remove_run(ctx: &ReducerContext, run_id: u64) {
    let continuations = ctx.db.game_creature_ai_relay_continuation();
    for row in continuations.by_run().filter(&run_id).collect::<Vec<_>>() {
        continuations.scheduled_id().delete(row.scheduled_id);
    }
    ctx.db.game_creature_ai_relay_run().id().delete(run_id);
}

fn cancel_run_tree(ctx: &ReducerContext, run_id: u64) {
    let children = ctx
        .db
        .game_creature_ai_relay_run()
        .by_parent()
        .filter(&run_id)
        .map(|run| run.id)
        .collect::<Vec<_>>();
    for child in children {
        cancel_run_tree(ctx, child);
    }
    remove_run(ctx, run_id);
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
        cancel_run_tree(ctx, run_id);
    }
    reap_unused_definitions(ctx);
}

pub(crate) fn cancel_relay_runs_for_source(ctx: &ReducerContext, source_guid: u64) {
    let run_ids = ctx
        .db
        .game_creature_ai_relay_run()
        .by_source()
        .filter(&source_guid)
        .filter(|run| source_lifetime_ends(run.lifetime))
        .map(|run| run.id)
        .collect::<Vec<_>>();
    for run_id in run_ids {
        cancel_run_tree(ctx, run_id);
    }
    reap_unused_definitions(ctx);
}

pub(crate) fn reap_unused_definitions(ctx: &ReducerContext) {
    let mut active_catalogues = ctx
        .db
        .game_creature_ai_relay_run()
        .iter()
        .map(|run| run.catalogue_version)
        .collect::<std::collections::HashSet<_>>();
    for start in ctx
        .db
        .game_creature_ai_definition()
        .iter()
        .flat_map(|definition| definition.rules)
        .flat_map(|rule| rule.instructions)
        .filter_map(|instruction| match instruction {
            super::CreatureInstruction::StartRelay(start) => Some(start),
            _ => None,
        })
    {
        active_catalogues.insert(start.catalogue_version);
    }
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
fn parse_forced_movement(value: &str) -> Result<RelayForcedMovement, String> {
    match value {
        "inherit" => Ok(RelayForcedMovement::Inherit),
        "walk" => Ok(RelayForcedMovement::Walk),
        "run" => Ok(RelayForcedMovement::Run),
        _ => Err(format!("unknown relay forced movement: {value}")),
    }
}
fn parse_flag_operation(value: &str) -> Result<RelayFlagOperation, String> {
    match value {
        "add" => Ok(RelayFlagOperation::Add),
        "remove" => Ok(RelayFlagOperation::Remove),
        "toggle" => Ok(RelayFlagOperation::Toggle),
        _ => Err(format!("unknown relay flag operation: {value}")),
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
            RelayInstruction::Emote(RelayEmote { emote_ids: vec![1] }),
        )
    }

    fn terminate() -> RelayInstruction {
        RelayInstruction::TerminateInvocation(RelayTerminate {
            subject: RelaySubject::Selected,
            when: RelayTerminateWhen::Missing,
        })
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
        let mut steps = [
            RelayStep {
                offset_ms: 20,
                command_priority: 2,
                source_order: 3,
                participants: RelayParticipants {
                    source: RelaySubject::Source,
                    target: RelaySubject::Selected,
                },
                instruction: terminate(),
            },
            RelayStep {
                offset_ms: 20,
                command_priority: 1,
                source_order: 9,
                participants: RelayParticipants {
                    source: RelaySubject::Source,
                    target: RelaySubject::Selected,
                },
                instruction: terminate(),
            },
            RelayStep {
                offset_ms: 20,
                command_priority: 1,
                source_order: 2,
                participants: RelayParticipants {
                    source: RelaySubject::Source,
                    target: RelaySubject::Selected,
                },
                instruction: terminate(),
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
    fn loader_accepts_bound_leaf_capabilities() {
        let graph = std::collections::BTreeMap::from([(
            1,
            vec![step(
                0,
                RelayInstruction::CastSpell(RelayCastSpell {
                    spell_ids: vec![1],
                    triggered: false,
                }),
            )],
        )]);
        assert!(validate_definition_graph(&graph).is_ok());
    }

    #[test]
    fn loader_refuses_unbound_leaf_capabilities() {
        let graph = std::collections::BTreeMap::from([(
            1,
            vec![step(
                0,
                RelayInstruction::ActivateObject(RelayActivateObject {
                    animation_id: 0,
                    custom_animation_id: 0,
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
    fn random_choices_advance_a_state_that_can_be_persisted_between_wakes() {
        let choices = [10, 20, 30];
        let mut uninterrupted = 77;
        let first = choose(&choices, &mut uninterrupted);
        let persisted_after_first = uninterrupted;
        let second = choose(&choices, &mut uninterrupted);

        let mut reloaded = persisted_after_first;
        assert_eq!(choose(&choices, &mut reloaded), second);
        assert_eq!(reloaded, uninterrupted);
        assert!(choices.contains(&first));
    }

    #[test]
    fn nested_runs_split_child_and_parent_random_states() {
        let initial = 91;
        let (child, parent) = next_random(initial);
        assert_ne!(child, parent);
        assert_ne!(parent, initial);
        assert_eq!(next_random(initial), (child, parent));
        assert_ne!(next_random(parent), (child, parent));
    }

    #[test]
    fn concurrency_dispositions_are_deterministic() {
        assert_eq!(
            start_disposition(RelayConcurrency::Parallel, vec![8, 3]),
            RelayStartDisposition::Start
        );
        assert_eq!(
            start_disposition(RelayConcurrency::IgnoreIfRunning, vec![]),
            RelayStartDisposition::Start
        );
        assert_eq!(
            start_disposition(RelayConcurrency::IgnoreIfRunning, vec![8, 3]),
            RelayStartDisposition::Existing(3)
        );
        assert_eq!(
            start_disposition(RelayConcurrency::Replace, vec![8, 3]),
            RelayStartDisposition::Replace(vec![3, 8])
        );
    }

    #[test]
    fn termination_and_source_lifetime_have_distinct_scopes() {
        assert!(termination_matches(RelayTerminateWhen::Present, true));
        assert!(termination_matches(RelayTerminateWhen::Missing, false));
        assert!(!termination_matches(RelayTerminateWhen::Present, false));
        assert!(!source_lifetime_ends(RelayLifetime::MapOrInstance));
        assert!(source_lifetime_ends(RelayLifetime::SourceCreature));
    }

    #[test]
    fn teardown_and_failure_paths_keep_the_durable_cleanup_wired() {
        let relay = include_str!("relay.rs");
        assert!(
            relay.contains("cancel_run_tree(ctx, run.id);\n        reap_unused_definitions(ctx);")
        );
        assert!(relay.contains(
            "cancel_run_tree(ctx, continuation.run_id);\n        reap_unused_definitions(ctx);"
        ));
        assert!(include_str!("../../instance.rs")
            .contains("crate::creatures::cancel_relay_runs_for_instance(ctx, instance_id)"));
        assert!(include_str!("../tick/lifecycle.rs")
            .contains("crate::creatures::cancel_relay_runs_for_source(ctx, guid)"));
        assert!(include_str!("edges.rs")
            .contains("super::cancel_relay_runs_for_source(ctx, payload.creature_guid)"));
    }

    #[test]
    fn dynamic_move_keeps_fixed_distance_and_advances_only_random_ranges() {
        let mut fixed_state = 12;
        assert_eq!(
            dynamic_destination((0.0, 0.0, 0.0), (10.0, 0.0, 2.0), 0, 0, 3, &mut fixed_state),
            (7.0, 0.0, 2.0)
        );
        assert_eq!(fixed_state, 12);

        let mut random_state = 12;
        let destination = dynamic_destination(
            (0.0, 0.0, 0.0),
            (10.0, 0.0, 2.0),
            2,
            4,
            0,
            &mut random_state,
        );
        assert!((6.0..=8.0).contains(&destination.0));
        assert_eq!(destination.1, 0.0);
        assert_eq!(destination.2, 2.0);
        assert_ne!(random_state, 12);
    }

    #[test]
    fn parser_keeps_guid_and_all_nearby_subjects_typed() {
        assert_eq!(
            parse_subject("creature-guid:44").unwrap(),
            RelaySubject::CreatureGuid(44)
        );
        assert_eq!(
            parse_subject("gameobject-guid:55").unwrap(),
            RelaySubject::GameObjectGuid(55)
        );
        assert_eq!(
            parse_subject("all-nearby-creatures:66:70").unwrap(),
            RelaySubject::AllNearbyCreatures(RelayNearby {
                entry: 66,
                radius_yd: 70,
            })
        );
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
