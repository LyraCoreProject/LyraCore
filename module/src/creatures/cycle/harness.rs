use super::*;
use crate::creatures::ai::ROUT_DURATION_MS;
use crate::creatures::eventai::{
    self, BroadcastLine, CallForHelpInstruction, CastInstruction, CreatureHealthCondition,
    CreatureInstruction, CreaturePresentationInstruction, CreatureReactState, CreatureState,
    CycleActor, DeathCondition, DefinitionRevision, EmoteInstruction, EngagedFight,
    EventAiDefinition, EventAiRequest, EventAiRule, EventAiSubject, EventAiUnit, EventAiWorld,
    EventCondition, EventContext, EventKind, EventPredicate, ExecutionPolicy,
    FriendlyHealthDeficitCondition, IdleMovementIntent, InstructionSelection, InstructionTarget,
    MovementOperation, NotifyEncounterInstruction, PhaseSet, PostureAdmission, RangedMode,
    RangedPostureInstruction, RecurrencePolicy, RuleState, SetPhaseInstruction, SpawnCondition,
    SpeakInstruction, SpeechMode, SpellCastTarget, SpellCasterAdmission, SpellCasterRole,
    SpellStartMode, SpellTargetRole, SummonInstruction, SummonLocation, TargetRangeCondition,
    TimeWindow, WalkingMode,
};
use crate::creatures::rout_window_open;
use crate::quest::{EventAiQuestCredit, EventAiQuestCreditContext, QuestCreditOutcome};
use crate::CreatureAiEvent;
use lyracore_shared::movement_flags::FORWARD;
use lyracore_shared::movement_path::chase_step;
use lyracore_shared::spatial;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};

#[path = "harness/eventai_combat.rs"]
mod eventai_combat;
#[path = "harness/eventai_death.rs"]
mod eventai_death;
#[path = "harness/eventai_edges.rs"]
mod eventai_edges;
#[path = "harness/eventai_instance_relay.rs"]
mod eventai_instance_relay;
#[path = "harness/eventai_legacy.rs"]
mod eventai_legacy;
#[path = "harness/eventai_mobility.rs"]
mod eventai_mobility;
#[path = "harness/eventai_movement.rs"]
mod eventai_movement;
#[path = "harness/eventai_profile.rs"]
mod eventai_profile;
#[path = "harness/eventai_quest_credit.rs"]
mod eventai_quest_credit;
#[path = "harness/eventai_tracer.rs"]
mod eventai_tracer;

pub(crate) use eventai_legacy::{
    effective_rule_id, ActionKind, Diagnostic, DiagnosticKind, RepeatPolicy, Rule, RuleAction,
    TargetPolicy, ACTION_CALL_FOR_HELP, ACTION_CAST, ACTION_EMOTE, ACTION_FLEE_FOR_ASSIST,
    ACTION_SAY, ACTION_SET_PHASE, ACTION_SET_RANGED_POSTURE, ACTION_SUMMON, ACTION_YELL,
    CAST_AURA_ABSENT, CAST_INTERRUPT_PREVIOUS, CAST_PLAYER_ONLY, CAST_TARGET_CASTING,
    CAST_TRIGGERED, EVENT_CREATURE_HP, EVENT_FRIENDLY_HP_DEFICIT, EVENT_ON_AGGRO, EVENT_ON_DEATH,
    EVENT_ON_SPAWN, EVENT_TARGET_RANGE, EVENT_TIMED_IN_COMBAT, REPEAT, REPEAT_ONCE,
    SOURCE_FLAG_COMBAT_ACTION, SOURCE_FLAG_RANDOM_ACTION, TARGET_AI_SENDER, TARGET_BENEFICIARY,
    TARGET_CURRENT, TARGET_EVENT, TARGET_FARTHEST_HOSTILE, TARGET_INVOKER, TARGET_NEAREST_AREA,
    TARGET_NO_EXPLICIT, TARGET_RANDOM_THREAT, TARGET_RANDOM_THREAT_PLAYER, TARGET_SECOND_THREAT,
    TARGET_SELF, TARGET_SPAWNER, TARGET_TOP_THREAT, TARGET_TOP_THREAT_PLAYER,
};

#[derive(Clone, Copy, PartialEq, Debug)]
struct XCreature {
    entry: u32,
    at: Point,
    grid: (i32, i32),
    cell: i64,
    last_move_ms: u32,
    orientation: f32,
    leg_ends_ms: u32,
    wp_target: u64,
    health: u32,
    max_health: u32,
    power: u32,
    max_power: u32,
    level: u32,
    faction_template: u32,
    map_id: u32,
    instance_id: u64,
    aggro_range: Option<u32>,
    detect_range_mod: f32,
    would_rout: bool,
    cannot_act: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct MoveEffect {
    guid: u64,
    start: Point,
    dest: Point,
    dur_ms: u32,
    spline_id: u32,
    run: bool,
    map_id: u32,
    instance_id: u64,
    grid: (i32, i32),
    cell: i64,
    facing: bool,
    facing_angle: f32,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum PetEffect {
    Took(u64, u64),
    StoodDown(u64),
    Dismissed(u64),
    Restaged(u64, Point, u32, u64),
    OrderCleared(u64),
}

#[derive(Clone, Copy)]
struct ScenarioSummon {
    at: Point,
    orientation: f32,
    lifetime_ms: u32,
}

#[derive(Clone, Copy)]
struct ScenarioSummonExpiry {
    lifetime_ms: u32,
    remaining_ms: u32,
    last_checked_ms: u64,
    summoner_guid: u64,
}

#[derive(Clone, Copy, Default)]
struct ScenarioMovement {
    creature_entry: u32,
    map_id: u32,
    instance_id: u64,
    idle: Option<IdleMovementIntent>,
    anchor: Option<Point>,
    patrol_paused: bool,
    combat_movement: Option<bool>,
    follow_movement: Option<bool>,
    walking: Option<WalkingMode>,
    immobilized: bool,
    immobilized_combat_only: bool,
    facing: Option<ScenarioFacing>,
}

#[derive(Clone, Copy)]
enum ScenarioFacing {
    Reset,
    Target(u64),
}

#[derive(Default)]
struct Scenario {
    creatures: RefCell<HashMap<u64, XCreature>>,
    legs: RefCell<Vec<LegInFlight>>,
    obstructed_paths: RefCell<HashSet<u64>>,
    frozen: RefCell<HashSet<u64>>,
    fears: RefCell<Vec<(u64, u64)>>,
    engaged: RefCell<HashSet<u64>>,
    routes: RefCell<HashMap<u64, Vec<Waypoint>>>,
    homes: RefCell<HashMap<u64, Home>>,
    now_micros: Cell<u64>,
    rolls: RefCell<VecDeque<u32>>,
    ground: Cell<Option<f32>>,
    detours: RefCell<HashMap<u64, (f32, f32)>>,
    effects: RefCell<Vec<MoveEffect>>,
    last_carrier: RefCell<HashMap<u64, (u64, MoveEffect)>>,
    players: RefCell<Vec<AggroTarget>>,
    eventai_player_health: RefCell<HashMap<u64, (u32, u32)>>,
    fights: RefCell<Vec<Engagement>>,
    rout_clock: RefCell<HashMap<u64, u32>>,
    hold_ranges: RefCell<HashMap<u64, f32>>,
    player_moved_ms: RefCell<HashMap<u64, u32>>,
    player_move_flags: RefCell<HashMap<u64, u32>>,
    at_war: RefCell<HashSet<(u32, u32)>>,
    blind: RefCell<HashSet<(u64, u64)>>,
    pulls: RefCell<Vec<(u64, u64, Pull)>>,
    flagged: RefCell<Vec<u64>>,
    combat_flags: RefCell<HashMap<u64, u64>>,
    unflagged: RefCell<Vec<u64>>,
    combat_regen: RefCell<HashSet<u64>>,
    rotations: RefCell<HashMap<u64, Vec<SpellOption>>>,
    creature_spell_lists: RefCell<HashMap<u64, Vec<SpellOption>>>,
    lone_spells: RefCell<HashMap<u64, u32>>,
    auras: RefCell<HashSet<(u64, u32)>>,
    casting: RefCell<HashSet<u64>>,
    channeling: RefCell<HashSet<u64>>,
    slowed: RefCell<HashSet<u64>>,
    not_ready: RefCell<HashSet<u32>>,
    eventai_spell_ranges: RefCell<HashMap<u32, u32>>,
    casts: RefCell<Vec<CastEffect>>,
    eventai_spell_starts:
        RefCell<Vec<(SpellStartMode, SpellCastTarget, bool, SpellCasterAdmission)>>,
    threat: RefCell<HashMap<(u64, u64), i64>>,
    taunts: RefCell<HashMap<u64, u64>>,
    pet_owners: RefCell<HashMap<u64, u64>>,
    pet_bars: RefCell<HashMap<u64, (PetOrder, PetReact)>>,
    corpses: RefCell<HashSet<u64>>,
    pet_effects: RefCell<Vec<PetEffect>>,
    awake: RefCell<TickSweep>,
    seen_by_sweep: RefCell<Vec<(u64, Point)>>,
    maintenance_runs: Cell<u32>,
    package_runs: Cell<u32>,
    eventai_rows: RefCell<Vec<CreatureAiEvent>>,
    eventai_definitions: RefCell<Vec<EventAiDefinition>>,
    eventai_text: RefCell<HashMap<u32, (String, u8, u32, u32)>>,
    eventai_text_sounds: RefCell<HashMap<u32, u32>>,
    eventai_creature_state: RefCell<HashMap<u64, CreatureState>>,
    eventai_rule_state: RefCell<HashMap<u64, HashMap<u64, RuleState>>>,
    eventai_presentation: RefCell<
        Vec<(
            u64,
            u64,
            DefinitionRevision,
            CreaturePresentationInstruction,
        )>,
    >,
    eventai_summons: RefCell<HashMap<u32, ScenarioSummon>>,
    eventai_templates: RefCell<HashSet<u32>>,
    eventai_exclude_caster_spells: RefCell<HashSet<u32>>,
    eventai_quests_taken: RefCell<HashSet<(u64, u32)>>,
    eventai_areas: RefCell<HashMap<u64, u32>>,
    eventai_returning_home: RefCell<HashSet<u64>>,
    eventai_movement: RefCell<HashMap<u64, ScenarioMovement>>,
    eventai_paths: RefCell<HashMap<(u32, u32, u32), Vec<Waypoint>>>,
    eventai_summon_expiry: RefCell<HashMap<u64, ScenarioSummonExpiry>>,
    eventai_lethal_floors: RefCell<HashMap<u64, DefinitionRevision>>,
    eventai_forced_deaths: RefCell<Vec<u64>>,
    eventai_forced_despawns: RefCell<Vec<(u64, u32)>>,
    eventai_ai_events: RefCell<Vec<(u64, u64, eventai::AiEventKind, u32)>>,
    eventai_stand_states: RefCell<HashMap<u64, u8>>,
    eventai_auto_attacks: RefCell<HashMap<u64, bool>>,
    eventai_sheaths: RefCell<HashMap<u64, u8>>,
    eventai_react_states: RefCell<HashMap<u64, CreatureReactState>>,
    eventai_encounter_notifications: RefCell<Vec<(u64, NotifyEncounterInstruction)>>,
    eventai_relay_starts: RefCell<Vec<(u32, u64, u64, u64)>>,
    eventai_next_summon: Cell<u64>,
    eventai_speech: RefCell<Vec<(u64, u8, String)>>,
    eventai_sounds: RefCell<Vec<(u64, u32, u8)>>,
    eventai_speech_targets: RefCell<Vec<(u64, u64)>>,
    eventai_emotes: RefCell<Vec<(u64, u32, u32, u64)>>,
    eventai_quest_credit_outcomes: RefCell<VecDeque<QuestCreditOutcome>>,
    eventai_quest_credit_results: RefCell<Vec<QuestCreditOutcome>>,
    eventai_diagnostics: RefCell<Vec<Diagnostic>>,
}

impl Scenario {
    fn new(now_micros: u64) -> Self {
        let s = Self::default();
        s.now_micros.set(now_micros);
        s
    }

    fn creature(self, guid: u64, at: Point) -> Self {
        let (gx, gy) = spatial::grid_cell(at.x, at.y);
        self.creatures.borrow_mut().insert(
            guid,
            XCreature {
                entry: guid as u32,
                at,
                grid: (gx, gy),
                cell: spatial::grid_cell_id(gx, gy),
                last_move_ms: 0,
                orientation: 0.0,
                leg_ends_ms: 0,
                wp_target: 0,
                health: 100,
                max_health: 100,
                power: 0,
                max_power: 0,
                level: 10,
                faction_template: BEASTS,
                map_id: MAP,
                instance_id: INSTANCE,
                aggro_range: Some(0),
                detect_range_mod: 0.0,
                would_rout: false,
                cannot_act: false,
            },
        );
        self
    }

    fn tweak(self, guid: u64, edit: impl FnOnce(&mut XCreature)) -> Self {
        edit(self.creatures.borrow_mut().get_mut(&guid).unwrap());
        self
    }

    fn level(self, guid: u64, level: u32) -> Self {
        self.tweak(guid, |c| c.level = level)
    }

    fn entry(self, guid: u64, entry: u32) -> Self {
        self.tweak(guid, |c| c.entry = entry)
    }

    fn eventai_row(self, row: CreatureAiEvent) -> Self {
        self.eventai_rows.borrow_mut().push(row);
        self
    }

    fn eventai_native_definition(self, definition: EventAiDefinition) -> Self {
        self.eventai_definitions.borrow_mut().push(definition);
        self
    }

    fn replace_eventai_definition(&self, definition: EventAiDefinition) {
        self.eventai_definitions
            .borrow_mut()
            .retain(|current| current.subject != definition.subject);
        self.eventai_definitions.borrow_mut().push(definition);
    }

    fn eventai_broadcast(self, id: u32, text: &str, chat_type: u8) -> Self {
        self.eventai_text
            .borrow_mut()
            .insert(id, (text.to_string(), chat_type, 0, 0));
        self
    }

    fn eventai_broadcast_emote(self, id: u32, text: &str, chat_type: u8, emote: u32) -> Self {
        self.eventai_text
            .borrow_mut()
            .insert(id, (text.to_string(), chat_type, 0, emote));
        self
    }

    fn eventai_summon(self, id: u32, at: Point, orientation: f32, lifetime_ms: u32) -> Self {
        self.eventai_summons.borrow_mut().insert(
            id,
            ScenarioSummon {
                at,
                orientation,
                lifetime_ms,
            },
        );
        self
    }

    fn eventai_template(self, entry: u32) -> Self {
        self.eventai_templates.borrow_mut().insert(entry);
        self
    }

    fn eventai_exclude_caster_spell(self, spell_id: u32) -> Self {
        self.eventai_exclude_caster_spells
            .borrow_mut()
            .insert(spell_id);
        self
    }

    fn eventai_quest_taken(self, guid: u64, quest_entry: u32) -> Self {
        self.eventai_quests_taken
            .borrow_mut()
            .insert((guid, quest_entry));
        self
    }

    fn eventai_summoned_guids(&self) -> Vec<u64> {
        let mut guids: Vec<u64> = self
            .eventai_summon_expiry
            .borrow()
            .keys()
            .copied()
            .collect();
        guids.sort_unstable();
        guids
    }

    fn reap_eventai_summons(&self) -> u64 {
        let now_ms = self.now_micros.get() / 1_000;
        let expiries: Vec<(u64, ScenarioSummonExpiry)> = self
            .eventai_summon_expiry
            .borrow()
            .iter()
            .map(|(guid, expiry)| (*guid, *expiry))
            .collect();
        for (guid, expiry) in &expiries {
            if !self.creatures.borrow().contains_key(guid) {
                self.clear_eventai_summon(*guid);
                continue;
            }
            let engaged = self
                .fights
                .borrow()
                .iter()
                .any(|fight| fight.attacker == *guid || fight.victim == *guid);
            let elapsed = now_ms
                .saturating_sub(expiry.last_checked_ms)
                .min(u64::from(u32::MAX)) as u32;
            match eventai::summon_lifetime_after(
                engaged,
                expiry.lifetime_ms,
                expiry.remaining_ms,
                elapsed,
            ) {
                None => self.clear_eventai_summon(*guid),
                Some(remaining) => {
                    if let Some(state) = self.eventai_summon_expiry.borrow_mut().get_mut(guid) {
                        state.remaining_ms = remaining;
                        state.last_checked_ms = now_ms;
                    }
                }
            }
        }
        expiries.len() as u64
    }

    fn clear_eventai_summon(&self, guid: u64) {
        self.fights
            .borrow_mut()
            .retain(|fight| fight.attacker != guid && fight.victim != guid);
        self.threat
            .borrow_mut()
            .retain(|pair, _| pair.0 != guid && pair.1 != guid);
        self.legs.borrow_mut().retain(|leg| leg.guid != guid);
        self.creatures.borrow_mut().remove(&guid);
        self.eventai_rule_state.borrow_mut().remove(&guid);
        self.eventai_creature_state.borrow_mut().remove(&guid);
        self.eventai_movement.borrow_mut().remove(&guid);
        self.eventai_summon_expiry.borrow_mut().remove(&guid);
    }

    fn eventai_speech(&self) -> Vec<(u64, u8, String)> {
        self.eventai_speech.borrow().clone()
    }

    fn eventai_speech_targets(&self) -> Vec<(u64, u64)> {
        self.eventai_speech_targets.borrow().clone()
    }

    fn eventai_emotes(&self) -> Vec<(u64, u32, u32, u64)> {
        self.eventai_emotes.borrow().clone()
    }

    fn eventai_lethal_floor(&self, guid: u64) -> Option<DefinitionRevision> {
        self.eventai_lethal_floors.borrow().get(&guid).copied()
    }

    fn eventai_forced_deaths(&self) -> Vec<u64> {
        self.eventai_forced_deaths.borrow().clone()
    }

    fn eventai_quest_credit_results(&self) -> Vec<QuestCreditOutcome> {
        self.eventai_quest_credit_results.borrow().clone()
    }

    fn eventai_quest_credit_outcome(self, outcome: QuestCreditOutcome) -> Self {
        self.eventai_quest_credit_outcomes
            .borrow_mut()
            .push_back(outcome);
        self
    }

    fn eventai_diagnostics(&self) -> Vec<Diagnostic> {
        self.eventai_diagnostics.borrow().clone()
    }

    fn movement_intent(&self, guid: u64) -> Option<ScenarioMovement> {
        let creature = self.creatures.borrow().get(&guid).copied()?;
        self.eventai_movement
            .borrow()
            .get(&guid)
            .copied()
            .filter(|intent| {
                intent.creature_entry == creature.entry
                    && intent.map_id == creature.map_id
                    && intent.instance_id == creature.instance_id
            })
    }

    fn eventai_state(&self, guid: u64, rule_id: u64) -> Option<RuleState> {
        self.eventai_rule_state
            .borrow()
            .get(&guid)
            .and_then(|rules| rules.get(&rule_id))
            .copied()
    }

    fn remove_eventai_rule(&self, rule_id: u64) {
        self.eventai_rows
            .borrow_mut()
            .retain(|row| effective_rule_id(row) != rule_id);
    }

    fn legacy_eventai_definitions(
        &self,
        creature_entry: u32,
        creature_guid: u64,
    ) -> Vec<EventAiDefinition> {
        let mut by_subject: BTreeMap<(u8, u64), BTreeMap<u64, Vec<CreatureAiEvent>>> =
            BTreeMap::new();
        for row in self.eventai_rows.borrow().iter().filter(|row| {
            (row.creature_guid == 0 && row.creature_entry == creature_entry)
                || (row.creature_entry == 0 && row.creature_guid == creature_guid)
        }) {
            let subject = if row.creature_entry != 0 {
                (0, u64::from(row.creature_entry))
            } else {
                (1, row.creature_guid)
            };
            by_subject
                .entry(subject)
                .or_default()
                .entry(effective_rule_id(row))
                .or_default()
                .push(row.clone());
        }

        by_subject
            .into_iter()
            .filter_map(|((kind, value), groups)| {
                let subject = if kind == 0 {
                    EventAiSubject::Entry(value as u32)
                } else {
                    EventAiSubject::Guid(value)
                };
                let mut rules = Vec::new();
                for rows in groups.into_values() {
                    match Rule::decode(rows) {
                        Ok(rule) => rules.push(native_test_rule(rule)),
                        Err(diagnostic) => self.eventai_diagnostics.borrow_mut().push(diagnostic),
                    }
                }
                if rules.is_empty() {
                    return None;
                }
                rules.sort_by_key(|rule| rule.source_rule_id);
                Some(EventAiDefinition {
                    subject,
                    revision: eventai::normalized_revision(subject, &rules),
                    rules,
                })
            })
            .collect()
    }

    fn authored_combat(&self, creature_guid: u64) -> eventai::AuthoredCombat {
        if self.pet_owners.borrow().contains_key(&creature_guid) {
            return eventai::AuthoredCombat::default();
        }
        let mut authored = eventai::AuthoredCombat::default();
        let definition = self.eventai_definition(creature_guid);
        for rule in definition.rules {
            if !rule.event.runs_while_engaged() {
                continue;
            }
            for instruction in rule.instructions {
                authored.casting |= matches!(instruction, CreatureInstruction::Cast(_));
                authored.main_spell_posture |= matches!(
                    instruction,
                    CreatureInstruction::Cast(CastInstruction {
                        main_spell: true,
                        ..
                    })
                );
                authored.flee |= matches!(
                    instruction,
                    CreatureInstruction::FleeForAssist | CreatureInstruction::FleeWithText(_)
                );
            }
        }
        authored
    }

    fn tuned_aggro_range(self, guid: u64, yards: u32) -> Self {
        self.tweak(guid, |c| c.aggro_range = Some(yards))
    }

    fn soothed(self, guid: u64, yards: f32) -> Self {
        self.tweak(guid, |c| c.detect_range_mod = yards)
    }

    fn near_death(self, guid: u64) -> Self {
        self.tweak(guid, |c| c.would_rout = true)
    }

    fn crowd_controlled(self, guid: u64) -> Self {
        self.tweak(guid, |c| c.cannot_act = true)
    }

    fn faction(self, guid: u64, faction_template: u32) -> Self {
        self.tweak(guid, |c| c.faction_template = faction_template)
    }

    fn in_instance(self, guid: u64, instance_id: u64) -> Self {
        self.tweak(guid, |c| c.instance_id = instance_id)
    }

    fn player(self, guid: u64, at: Point) -> Self {
        self.players.borrow_mut().push(AggroTarget {
            guid,
            at,
            level: 10,
            faction_template: ALLIANCE,
            map_id: MAP,
            instance_id: INSTANCE,
            dead: false,
            godmode: false,
            taxi_flying: false,
            stealthed: false,
        });
        self
    }

    fn tweak_player(self, guid: u64, edit: impl FnOnce(&mut AggroTarget)) -> Self {
        edit(
            self.players
                .borrow_mut()
                .iter_mut()
                .find(|p| p.guid == guid)
                .unwrap(),
        );
        self
    }

    fn player_level(self, guid: u64, level: u32) -> Self {
        self.tweak_player(guid, |p| p.level = level)
    }

    fn player_health(self, guid: u64, health: u32, max_health: u32) -> Self {
        self.eventai_player_health
            .borrow_mut()
            .insert(guid, (health, max_health));
        self
    }

    fn godmoded(self, guid: u64) -> Self {
        self.tweak_player(guid, |p| p.godmode = true)
    }

    fn corpse(self, guid: u64) -> Self {
        self.tweak_player(guid, |p| p.dead = true)
    }

    fn stealthed(self, guid: u64) -> Self {
        self.tweak_player(guid, |p| p.stealthed = true)
    }

    fn at_war(self, a: u32, b: u32) -> Self {
        self.at_war.borrow_mut().extend([(a, b), (b, a)]);
        self
    }

    fn wall_between(self, looker: u64, unseen: u64) -> Self {
        self.blind.borrow_mut().insert((looker, unseen));
        self
    }

    fn attacking(self, attacker: u64, victim: u64) -> Self {
        self.fights.borrow_mut().push(Engagement {
            attacker,
            victim,
            instance_id: INSTANCE,
            player_never_swung: false,
        });
        self
    }

    fn routing(self, guid: u64) -> Self {
        let ends_ms = (self.now_micros.get() / 1000) as u32 + ROUT_DURATION_MS;
        self.rout_clock.borrow_mut().insert(guid, ends_ms);
        self.wounded_runner(guid)
    }

    fn wounded_runner(self, guid: u64) -> Self {
        self.hurt(guid, 10).near_death(guid)
    }

    fn feared_by(self, guid: u64, caster: u64) -> Self {
        self.fears.borrow_mut().push((guid, caster));
        self
    }

    fn caster(self, guid: u64, yards: f32) -> Self {
        self.hold_ranges.borrow_mut().insert(guid, yards);
        self
    }

    fn rotation_line(self, guid: u64, spell_id: u32, when: CastWhen, priority: u8) -> Self {
        let mut rotations = self.rotations.borrow_mut();
        let lines = rotations.entry(guid).or_default();
        lines.push(SpellOption {
            spell_id,
            when,
            priority,
            authored: lines.len() as u64,
        });
        drop(rotations);
        self
    }

    fn creature_spell_list_line(
        self,
        guid: u64,
        spell_id: u32,
        when: CastWhen,
        priority: u8,
    ) -> Self {
        let mut lists = self.creature_spell_lists.borrow_mut();
        let lines = lists.entry(guid).or_default();
        lines.push(SpellOption {
            spell_id,
            when,
            priority,
            authored: lines.len() as u64,
        });
        drop(lists);
        self
    }

    fn lone_spell(self, guid: u64, spell_id: u32) -> Self {
        self.lone_spells.borrow_mut().insert(guid, spell_id);
        self
    }

    fn carrying(self, guid: u64, spell_id: u32) -> Self {
        self.auras.borrow_mut().insert((guid, spell_id));
        self
    }

    fn mid_cast(self, guid: u64) -> Self {
        self.casting.borrow_mut().insert(guid);
        self
    }

    fn channeling(self, guid: u64) -> Self {
        self.channeling.borrow_mut().insert(guid);
        self
    }

    fn not_ready(self, spell_id: u32) -> Self {
        self.not_ready.borrow_mut().insert(spell_id);
        self
    }

    fn spell_range(self, spell_id: u32, range_yd: u32) -> Self {
        self.eventai_spell_ranges
            .borrow_mut()
            .insert(spell_id, range_yd);
        self
    }

    fn hurt(self, guid: u64, health: u32) -> Self {
        self.tweak(guid, |c| c.health = health)
    }

    fn threat(self, creature: u64, source: u64, threat: i64) -> Self {
        self.threat.borrow_mut().insert((creature, source), threat);
        self
    }

    fn threat_value(&self, creature: u64, source: u64) -> Option<i64> {
        self.threat.borrow().get(&(creature, source)).copied()
    }

    fn taunted(self, creature: u64, taunter: u64) -> Self {
        self.taunts.borrow_mut().insert(creature, taunter);
        self
    }

    fn casts(&self) -> Vec<CastEffect> {
        self.casts.borrow().clone()
    }

    fn victims(&self) -> Vec<(u64, u64)> {
        let mut pairs: Vec<(u64, u64)> = self
            .fights
            .borrow()
            .iter()
            .map(|f| (f.attacker, f.victim))
            .collect();
        pairs.sort_unstable();
        pairs
    }

    fn kiting(self, guid: u64, at: Point) -> Self {
        let now_ms = (self.now_micros.get() / 1000) as u32;
        self.player_moved_ms.borrow_mut().insert(guid, now_ms);
        self.player_move_flags.borrow_mut().insert(guid, FORWARD);
        self.tweak_player(guid, |p| p.at = at)
    }

    fn stopped(self, guid: u64) -> Self {
        self.player_move_flags.borrow_mut().insert(guid, 0);
        self
    }

    fn turning(self, guid: u64) -> Self {
        let now_ms = (self.now_micros.get() / 1000) as u32;
        self.player_moved_ms.borrow_mut().insert(guid, now_ms);
        self.player_move_flags
            .borrow_mut()
            .insert(guid, MOVE_FLAG_TURN_LEFT);
        self
    }

    fn since_last_move_ms(&self, guid: u64) -> u32 {
        let now_ms = (self.now_micros.get() / 1000) as u32;
        now_ms.wrapping_sub(
            self.player_moved_ms
                .borrow()
                .get(&guid)
                .copied()
                .unwrap_or(0),
        )
    }

    fn facing(self, guid: u64, orientation: f32) -> Self {
        self.tweak(guid, |c| c.orientation = orientation)
    }

    fn aiming_at(self, player: u64, victim: u64) -> Self {
        self.fights.borrow_mut().push(Engagement {
            attacker: player,
            victim,
            instance_id: INSTANCE,
            player_never_swung: true,
        });
        self
    }

    fn pulls(&self) -> Vec<(u64, u64, Pull)> {
        self.pulls.borrow().clone()
    }

    fn flagged(&self) -> Vec<u64> {
        self.flagged.borrow().clone()
    }

    fn in_combat_until(self, guid: u64, until_ms: u64) -> Self {
        self.combat_flags.borrow_mut().insert(guid, until_ms);
        self.awake.borrow_mut().in_combat.push(guid);
        self
    }

    fn in_combat(&self, guid: u64) -> bool {
        self.combat_flags.borrow().contains_key(&guid)
    }

    fn unflagged(&self) -> Vec<u64> {
        self.unflagged.borrow().clone()
    }

    fn power(self, guid: u64, power: u32, max_power: u32) -> Self {
        self.tweak(guid, |c| {
            c.power = power;
            c.max_power = max_power;
        })
    }

    fn combat_regen(self, guid: u64) -> Self {
        self.combat_regen.borrow_mut().insert(guid);
        self
    }

    fn pet(self, guid: u64, owner: u64, at: Point) -> Self {
        let s = self.creature(guid, at);
        s.awake.borrow_mut().pets.push(guid);
        s.pet_owners.borrow_mut().insert(guid, owner);
        s
    }

    fn pet_bar(self, owner: u64, command: PetOrder, react: PetReact) -> Self {
        self.pet_bars.borrow_mut().insert(owner, (command, react));
        self
    }

    fn slain(self, guid: u64) -> Self {
        self.corpses.borrow_mut().insert(guid);
        self
    }

    fn pet_effects(&self) -> Vec<PetEffect> {
        self.pet_effects.borrow().clone()
    }

    fn awake(self, guids: impl IntoIterator<Item = u64>) -> Self {
        self.awake.borrow_mut().active = guids.into_iter().collect();
        self
    }

    fn route(self, guid: u64, waypoints: &[(u64, Point)]) -> Self {
        self.routes.borrow_mut().insert(
            guid,
            waypoints
                .iter()
                .map(|(id, at)| Waypoint { id: *id, at: *at })
                .collect(),
        );
        self
    }

    fn walking_to(self, guid: u64, waypoint_id: u64) -> Self {
        self.creatures
            .borrow_mut()
            .get_mut(&guid)
            .unwrap()
            .wp_target = waypoint_id;
        self
    }

    fn home(self, guid: u64, at: Point, wanders: bool) -> Self {
        self.homes.borrow_mut().insert(guid, Home { at, wanders });
        self
    }

    fn fighting(self, guid: u64) -> Self {
        self.engaged.borrow_mut().insert(guid);
        self
    }

    fn mid_leg(self, guid: u64, leg_ends_ms: u32) -> Self {
        self.creatures
            .borrow_mut()
            .get_mut(&guid)
            .unwrap()
            .leg_ends_ms = leg_ends_ms;
        self
    }

    fn rolls(self, rolls: impl IntoIterator<Item = u32>) -> Self {
        *self.rolls.borrow_mut() = rolls.into_iter().collect();
        self
    }

    fn ground(self, z: f32) -> Self {
        self.ground.set(Some(z));
        self
    }

    fn detour(self, guid: u64, corner: (f32, f32)) -> Self {
        self.detours.borrow_mut().insert(guid, corner);
        self
    }

    fn advance_clock(&self, micros: u64) {
        self.now_micros.set(self.now_micros.get() + micros);
    }

    fn flying(
        self,
        guid: u64,
        start: Point,
        dest: Point,
        started_micros: u64,
        dur_ms: u32,
    ) -> Self {
        let partition = self
            .creatures
            .borrow()
            .get(&guid)
            .map(|creature| (creature.map_id, creature.instance_id))
            .unwrap_or((MAP, INSTANCE));
        self.legs.borrow_mut().push(LegInFlight {
            guid,
            start,
            dest,
            started_micros,
            dur_ms,
            map_id: partition.0,
            instance_id: partition.1,
            back_off_from: 0,
            mover_gone: !self.creatures.borrow().contains_key(&guid),
            waypoints: Vec::new(),
        });
        self
    }

    fn rooted(self, guid: u64) -> Self {
        self.frozen.borrow_mut().insert(guid);
        self
    }

    fn slowed(self, guid: u64) -> Self {
        self.slowed.borrow_mut().insert(guid);
        self
    }

    fn rout_ends_ms(&self, guid: u64) -> u32 {
        self.rout_clock.borrow().get(&guid).copied().unwrap_or(0)
    }

    fn at(&self, guid: u64) -> XCreature {
        self.creatures.borrow()[&guid]
    }

    fn has_leg(&self, guid: u64) -> bool {
        self.legs.borrow().iter().any(|l| l.guid == guid)
    }

    fn effects(&self) -> Vec<MoveEffect> {
        self.effects.borrow().clone()
    }

    fn tick(&self, sense: bool, scope: TickScope) -> TickContext {
        TickContext {
            now_micros: self.now_micros.get(),
            now_ms: (self.now_micros.get() / 1000) as u32,
            tick_secs: crate::creatures::MOVE_TICK_SECS,
            sense,
            sense_secs: crate::creatures::MOVE_TICK_SECS
                * crate::creatures::SENSE_EVERY_N_TICKS as f32,
            scope,
        }
    }

    fn firing(&self, now_micros: u64, interval_micros: i64, scope: TickScope) -> TickContext {
        self.now_micros.set(now_micros);
        TickContext {
            now_micros,
            now_ms: (now_micros / 1000) as u32,
            tick_secs: crate::creatures::tick_secs_for_interval(interval_micros),
            sense: crate::creatures::is_sense_tick_for_interval(now_micros as i64, interval_micros),
            sense_secs: crate::creatures::sense_period_secs_for_interval(interval_micros),
            scope,
        }
    }
}

impl MotionSink for Scenario {
    fn path_obstruction(&self, leg: &LegInFlight, _now_micros: u64) -> Option<Point> {
        self.obstructed_paths
            .borrow()
            .contains(&leg.guid)
            .then(|| self.at(leg.guid).at)
    }
    fn legs_in_flight(&self, scope: &TickScope) -> Vec<LegInFlight> {
        self.legs
            .borrow()
            .iter()
            .filter(|leg| scope.covers(leg.instance_id))
            .cloned()
            .collect()
    }
    fn movement_suppressed(&self, guid: u64) -> bool {
        self.cc(guid).2
            || self
                .movement_intent(guid)
                .is_some_and(|intent| intent.immobilized)
    }
    fn commit_position(&mut self, guid: u64, at: Point, moved_ms: u32) {
        self.place(guid, at, Some(moved_ms), None);
    }
    fn halt(&mut self, leg: &LegInFlight, at: Point) {
        let Some(c) = self.place(leg.guid, at, None, None) else {
            return;
        };
        self.record(MoveEffect {
            guid: leg.guid,
            start: at,
            dest: at,
            dur_ms: 0,
            spline_id: self.next_spline_id(leg.guid),
            run: false,
            map_id: leg.map_id,
            instance_id: leg.instance_id,
            grid: c.grid,
            cell: c.cell,
            facing: false,
            facing_angle: 0.0,
        });
        self.park_leg(leg.guid, at, leg.map_id, leg.instance_id);
    }
    fn drop_leg(&mut self, guid: u64) {
        self.legs.borrow_mut().retain(|l| l.guid != guid);
    }
}

impl Scenario {
    fn cc(&self, guid: u64) -> (bool, bool, bool) {
        let frozen = self.frozen.borrow().contains(&guid);
        let feared = self.fears.borrow().iter().any(|(unit, _)| *unit == guid);
        crate::spell::cc_blocks(frozen, false, feared, false)
    }

    fn is_routing(&self, guid: u64) -> bool {
        let entitled = self
            .creatures
            .borrow()
            .get(&guid)
            .is_some_and(|c| c.would_rout)
            || self.authored_combat(guid).flee;
        let now_ms = (self.now_micros.get() / 1000) as u32;
        entitled && rout_window_open(now_ms, self.rout_ends_ms(guid)) && !self.cc(guid).2
    }

    fn place(
        &self,
        guid: u64,
        at: Point,
        moved_ms: Option<u32>,
        orientation: Option<f32>,
    ) -> Option<XCreature> {
        let mut creatures = self.creatures.borrow_mut();
        let c = creatures.get_mut(&guid)?;
        let (gx, gy) = spatial::grid_cell(at.x, at.y);
        c.at = at;
        c.grid = (gx, gy);
        c.cell = spatial::grid_cell_id(gx, gy);
        if let Some(ms) = moved_ms {
            c.last_move_ms = ms;
        }
        if let Some(rad) = orientation {
            c.orientation = rad;
        }
        Some(*c)
    }

    fn next_spline_id(&self, guid: u64) -> u32 {
        let now_ms = (self.now_micros.get() / 1000) as u32;
        self.last_carrier
            .borrow()
            .get(&guid)
            .map_or(now_ms, |(_, previous)| {
                crate::creatures::leg::next_spline_id(now_ms, previous.spline_id)
            })
    }

    fn record(&self, effect: MoveEffect) {
        let now = self.now_micros.get();
        if let Some((wrote_at, first)) = self.last_carrier.borrow().get(&effect.guid) {
            assert!(
                *wrote_at != now,
                "guid {} wrote two carrier rows in one firing: {first:?} then {effect:?}",
                effect.guid
            );
        }
        self.last_carrier
            .borrow_mut()
            .insert(effect.guid, (now, effect));
        self.effects.borrow_mut().push(effect);
    }

    fn park_leg(&self, guid: u64, at: Point, map_id: u32, instance_id: u64) {
        let mut legs = self.legs.borrow_mut();
        legs.retain(|l| l.guid != guid);
        legs.push(LegInFlight {
            guid,
            start: at,
            dest: at,
            started_micros: self.now_micros.get(),
            dur_ms: 0,
            map_id,
            instance_id,
            back_off_from: 0,
            mover_gone: false,
            waypoints: Vec::new(),
        });
    }

    fn unit(&self, guid: u64) -> Option<(Point, u32)> {
        if let Some(c) = self.creatures.borrow().get(&guid) {
            let carried = self
                .legs
                .borrow()
                .iter()
                .any(|l| l.guid == guid && l.dur_ms > 0);
            return Some((c.at, if carried { FORWARD } else { 0 }));
        }
        let flags = self.player_move_flags.borrow().get(&guid).copied();
        self.players
            .borrow()
            .iter()
            .find(|p| p.guid == guid)
            .map(|p| (p.at, flags.unwrap_or(0)))
    }

    fn pet_owner(&self, guid: u64) -> Option<PetOwner> {
        let (at, map_id, instance_id) = self
            .players
            .borrow()
            .iter()
            .find(|p| p.guid == guid && !p.dead)
            .map(|p| (p.at, p.map_id, p.instance_id))
            .or_else(|| {
                self.creatures
                    .borrow()
                    .get(&guid)
                    .filter(|_| !self.corpses.borrow().contains(&guid))
                    .map(|c| (c.at, c.map_id, c.instance_id))
            })?;
        let combat_target = self
            .fights
            .borrow()
            .iter()
            .find(|f| f.attacker == guid)
            .map(|f| f.victim)
            .unwrap_or(0);
        Some(PetOwner {
            guid,
            at,
            map_id,
            instance_id,
            combat_target,
        })
    }

    fn snapshot(&self) -> Vec<(u64, Point)> {
        let mut seen: Vec<(u64, Point)> = self
            .creatures
            .borrow()
            .iter()
            .map(|(guid, c)| (*guid, c.at))
            .collect();
        seen.sort_by_key(|(guid, _)| *guid);
        seen
    }
}

fn native_test_rule(rule: Rule) -> EventAiRule {
    let event = match rule.event {
        EventKind::OnAggro => EventCondition::OnAggro,
        EventKind::TimedInCombat => EventCondition::TimedInCombat(TimeWindow {
            min_ms: rule.event_params[0],
            max_ms: rule.event_params[1],
        }),
        EventKind::CreatureHp => EventCondition::CreatureHealth(CreatureHealthCondition {
            min_pct: rule.event_params[0] as u8,
            max_pct: rule.event_params[1] as u8,
            allow_out_of_combat: false,
        }),
        EventKind::OnDeath => EventCondition::OnDeath(DeathCondition {
            predicate: EventPredicate::Always,
        }),
        EventKind::TargetRange => EventCondition::TargetRange(TargetRangeCondition {
            min_yd: rule.event_params[0],
            max_yd: rule.event_params[1],
        }),
        EventKind::OnSpawn => EventCondition::OnSpawn(SpawnCondition::Always),
        EventKind::FriendlyHpDeficit => {
            EventCondition::FriendlyHealthDeficit(FriendlyHealthDeficitCondition {
                missing_health: rule.event_params[0],
                radius_yd: rule.event_params[1],
                percent: false,
            })
        }
        _ => unreachable!("the retired flat fixture cannot produce this event kind"),
    };
    EventAiRule {
        source_rule_id: rule.id,
        event,
        chance_pct: rule.chance_pct,
        allowed_phases: PhaseSet {
            bits: rule.allowed_phase_mask,
        },
        recurrence: match rule.repeat {
            RepeatPolicy::Once => RecurrencePolicy::Once,
            RepeatPolicy::Repeat => RecurrencePolicy::Repeat(TimeWindow {
                min_ms: rule.event_params[2],
                max_ms: rule.event_params[3],
            }),
        },
        selection: if rule.source_policy.contains(SOURCE_FLAG_RANDOM_ACTION) {
            InstructionSelection::RandomOne
        } else {
            InstructionSelection::All
        },
        execution: if rule.source_policy.contains(SOURCE_FLAG_COMBAT_ACTION) {
            ExecutionPolicy::CombatAction
        } else {
            ExecutionPolicy::Ordinary
        },
        posture: PostureAdmission::Any,
        instructions: rule.actions.iter().map(native_test_instruction).collect(),
    }
}

fn native_test_instruction(action: &RuleAction) -> CreatureInstruction {
    let target = match action.target {
        TargetPolicy::Current => InstructionTarget::CurrentOpponent,
        TargetPolicy::SelfActor => InstructionTarget::SelfActor,
        TargetPolicy::TopThreat => InstructionTarget::HighestThreat,
        TargetPolicy::SecondThreat => InstructionTarget::SecondThreat,
        TargetPolicy::RandomThreat => InstructionTarget::RandomThreat,
        TargetPolicy::Invoker => InstructionTarget::Invoker,
        TargetPolicy::EventTarget => InstructionTarget::EventSubject,
        TargetPolicy::TopThreatPlayer => InstructionTarget::HighestThreatCharacter,
        TargetPolicy::RandomThreatPlayer => InstructionTarget::RandomThreatCharacter,
        TargetPolicy::NearestArea => InstructionTarget::EligibleCasterArea,
        TargetPolicy::FarthestHostile => InstructionTarget::FarthestHostile,
        TargetPolicy::Beneficiary => InstructionTarget::Beneficiary,
        TargetPolicy::AiSender => InstructionTarget::AiSender,
        TargetPolicy::Spawner => InstructionTarget::Spawner,
        TargetPolicy::NoExplicit => InstructionTarget::NoExplicitSpellTarget,
    };
    match action.kind {
        ActionKind::Say | ActionKind::Yell => CreatureInstruction::Speak(SpeakInstruction {
            mode: if action.kind == ActionKind::Yell {
                SpeechMode::Yell
            } else {
                SpeechMode::Say
            },
            broadcast_ids: action.params.into_iter().filter(|id| *id != 0).collect(),
            legacy_text: action.legacy_text.clone(),
            target,
        }),
        ActionKind::Cast => CreatureInstruction::Cast(CastInstruction {
            spell_id: action.params[0],
            target,
            interrupt_previous: action.cast_options.contains(CAST_INTERRUPT_PREVIOUS),
            start_mode: if action.cast_options.contains(CAST_TRIGGERED) {
                SpellStartMode::Triggered
            } else {
                SpellStartMode::Direct
            },
            caster_role: SpellCasterRole::Actor,
            target_role: SpellTargetRole::Selected,
            aura_absent: action.cast_options.contains(CAST_AURA_ABSENT),
            character_only: action.cast_options.contains(CAST_PLAYER_ONLY),
            target_must_be_casting: action.cast_options.contains(CAST_TARGET_CASTING),
            main_spell: false,
            distance_after_start: false,
        }),
        ActionKind::Emote => CreatureInstruction::Emote(EmoteInstruction {
            emote_id: action.params[0],
            target,
        }),
        ActionKind::FleeForAssist => CreatureInstruction::FleeForAssist,
        ActionKind::CallForHelp => CreatureInstruction::CallForHelp(CallForHelpInstruction {
            radius_yd: action.params[0],
        }),
        ActionKind::SetPhase => CreatureInstruction::SetPhase(SetPhaseInstruction {
            phase: action.params[0] as u8,
        }),
        ActionKind::Summon => CreatureInstruction::Summon(SummonInstruction {
            creature_entry: action.params[0],
            summon_location_id: action.params[2],
            target,
        }),
        ActionKind::SetRangedPosture => {
            CreatureInstruction::SetRangedPosture(RangedPostureInstruction {
                distance_yd: action.params[0],
                angle_degrees: action.params[1] as i32,
            })
        }
    }
}

impl CreatureWorld for Scenario {
    fn awake_creatures(&self, _scope: &TickScope, _sense: bool) -> TickSweep {
        *self.seen_by_sweep.borrow_mut() = self.snapshot();
        let awake = self.awake.borrow();
        TickSweep {
            active: awake.active.clone(),
            awake: awake.active.clone(),
            pets: awake.pets.clone(),
            in_combat: awake.in_combat.clone(),
        }
    }
    fn run_due_world_maintenance(&mut self) -> Vec<(&'static str, u64)> {
        self.maintenance_runs.set(self.maintenance_runs.get() + 1);
        let summons = self.reap_eventai_summons();
        if summons == 0 {
            Vec::new()
        } else {
            vec![("eventai_summon_expiry*", summons)]
        }
    }
    fn evaluate_eventai(&mut self, request: EventAiRequest<'_>) -> u64 {
        eventai::evaluate(self, request)
    }
    fn run_package_passes(&mut self) {
        self.package_runs.set(self.package_runs.get() + 1);
    }
}

impl EventAiWorld for Scenario {
    fn eventai_now_ms(&self) -> u64 {
        self.now_micros.get() / 1000
    }

    fn eventai_fights(&self, scope: &TickScope) -> Vec<EngagedFight> {
        self.fights
            .borrow()
            .iter()
            .filter(|fight| {
                self.creatures
                    .borrow()
                    .get(&fight.attacker)
                    .is_some_and(|creature| scope.covers(creature.instance_id))
            })
            .map(|fight| EngagedFight {
                creature_guid: fight.attacker,
                victim_guid: fight.victim,
            })
            .collect()
    }

    fn eventai_cycle_actors(&self, scope: &TickScope, active: &HashSet<u64>) -> Vec<CycleActor> {
        let mut actors = BTreeMap::new();
        for guid in active {
            let Some(creature) = self.creatures.borrow().get(guid).copied() else {
                continue;
            };
            if !scope.covers(creature.instance_id) || self.corpses.borrow().contains(guid) {
                continue;
            }
            let target = self
                .fights
                .borrow()
                .iter()
                .find(|fight| fight.attacker == *guid)
                .map(|fight| fight.victim);
            actors.insert(
                *guid,
                CycleActor {
                    creature_guid: *guid,
                    current_target_guid: target,
                    engaged: target.is_some(),
                },
            );
        }
        for fight in self.eventai_fights(scope) {
            actors.entry(fight.creature_guid).or_insert(CycleActor {
                creature_guid: fight.creature_guid,
                current_target_guid: Some(fight.victim_guid),
                engaged: true,
            });
        }
        actors.into_values().collect()
    }

    fn eventai_unit(&self, guid: u64) -> Option<EventAiUnit> {
        if let Some(c) = self.creatures.borrow().get(&guid) {
            return Some(EventAiUnit {
                guid,
                entry: c.entry,
                x: c.at.x,
                y: c.at.y,
                z: c.at.z,
                map_id: c.map_id,
                instance_id: c.instance_id,
                zone_id: 0,
                health: c.health,
                max_health: c.max_health,
                power: c.power,
                max_power: c.max_power,
                power_type: lyracore_shared::packing::power_type::MANA,
                level: c.level,
                faction_template: c.faction_template,
                dead: self.corpses.borrow().contains(&guid),
                is_player: false,
                orientation: c.orientation,
                owner_guid: self.pet_owners.borrow().get(&guid).copied().unwrap_or(0),
            });
        }
        self.players
            .borrow()
            .iter()
            .find(|p| p.guid == guid)
            .map(|p| EventAiUnit {
                guid,
                entry: 0,
                x: p.at.x,
                y: p.at.y,
                z: p.at.z,
                map_id: p.map_id,
                instance_id: p.instance_id,
                zone_id: 0,
                health: self
                    .eventai_player_health
                    .borrow()
                    .get(&guid)
                    .map_or(1, |vitals| vitals.0),
                max_health: self
                    .eventai_player_health
                    .borrow()
                    .get(&guid)
                    .map_or(1, |vitals| vitals.1),
                power: 100,
                max_power: 100,
                power_type: lyracore_shared::packing::power_type::MANA,
                level: p.level,
                faction_template: p.faction_template,
                dead: p.dead,
                is_player: true,
                orientation: 0.0,
                owner_guid: 0,
            })
    }

    fn eventai_spawner_guid(&self, creature_guid: u64) -> Option<u64> {
        self.eventai_summon_expiry
            .borrow()
            .get(&creature_guid)
            .map(|expiry| expiry.summoner_guid)
    }

    fn eventai_units_near(&self, center: &EventAiUnit, _radius_yd: f32) -> Vec<EventAiUnit> {
        let mut guids: Vec<u64> = self.creatures.borrow().keys().copied().collect();
        guids.extend(self.players.borrow().iter().map(|p| p.guid));
        guids.sort_unstable();
        guids
            .into_iter()
            .filter_map(|guid| self.eventai_unit(guid))
            .filter(|unit| unit.map_id == center.map_id && unit.instance_id == center.instance_id)
            .collect()
    }

    fn eventai_threat(&self, creature_guid: u64) -> Vec<(u64, i64)> {
        crate::threat::rank(
            self.threat
                .borrow()
                .iter()
                .filter(|((owner, _), _)| *owner == creature_guid)
                .map(|((_, source), threat)| (*source, *threat))
                .collect(),
        )
    }

    fn eventai_scale_selected_threat(&mut self, operation: crate::threat::ScaleSelectedThreat) {
        let mut threats = self.threat.borrow_mut();
        if let Some(threat) = threats.get_mut(&(operation.creature_guid, operation.source_guid)) {
            *threat = crate::threat::threat_after_percent_scale(*threat, operation.percent);
        }
    }

    fn eventai_scale_all_threat(&mut self, operation: crate::threat::ScaleAllThreat) {
        let mut threats = self.threat.borrow_mut();
        for ((creature_guid, _), threat) in threats.iter_mut() {
            if *creature_guid == operation.creature_guid {
                *threat = crate::threat::threat_after_percent_scale(*threat, operation.percent);
            }
        }
    }

    fn eventai_has_aura(&self, guid: u64, spell_id: u32) -> bool {
        self.auras.borrow().contains(&(guid, spell_id))
    }

    fn eventai_aura_stacks(&self, guid: u64, spell_id: u32) -> u32 {
        u32::from(self.eventai_has_aura(guid, spell_id))
    }

    fn eventai_is_crowd_controlled(&self, guid: u64, _dispel_type: u32) -> bool {
        self.frozen.borrow().contains(&guid)
            || self.slowed.borrow().contains(&guid)
            || self
                .creatures
                .borrow()
                .get(&guid)
                .is_some_and(|creature| creature.cannot_act)
    }

    fn eventai_is_casting(&self, guid: u64) -> bool {
        self.casting.borrow().contains(&guid) || self.channeling.borrow().contains(&guid)
    }

    fn eventai_factions_friendly(&self, first: u32, second: u32) -> bool {
        first == second
    }

    fn eventai_factions_hostile(&self, first: u32, second: u32) -> bool {
        self.at_war.borrow().contains(&(first, second))
    }

    fn eventai_line_of_sight(&self, first: &EventAiUnit, second: &EventAiUnit) -> bool {
        !self.blind.borrow().contains(&(first.guid, second.guid))
    }

    fn eventai_spell_range(&self, spell_id: u32) -> Option<u32> {
        Some(
            self.eventai_spell_ranges
                .borrow()
                .get(&spell_id)
                .copied()
                .unwrap_or(u32::MAX),
        )
    }

    fn eventai_spell_excludes_caster(&self, spell_id: u32) -> bool {
        self.eventai_exclude_caster_spells
            .borrow()
            .contains(&spell_id)
    }

    fn eventai_is_engaged(&self, guid: u64) -> bool {
        self.fights
            .borrow()
            .iter()
            .any(|fight| fight.attacker == guid || fight.victim == guid)
    }

    fn eventai_is_evading(&self, guid: u64) -> bool {
        self.eventai_returning_home.borrow().contains(&guid)
    }

    fn eventai_matches_predicate(&self, guid: u64, predicate: EventPredicate) -> bool {
        match predicate {
            EventPredicate::Always => true,
            EventPredicate::Alliance => self
                .players
                .borrow()
                .iter()
                .any(|player| player.guid == guid),
            EventPredicate::Horde => false,
            EventPredicate::QuestTaken(quest) => self
                .eventai_quests_taken
                .borrow()
                .contains(&(guid, quest.quest_entry)),
            EventPredicate::QuestIncomplete(quest_entry) => self
                .eventai_quests_taken
                .borrow()
                .contains(&(guid, quest_entry)),
            EventPredicate::Map(map_id) => self
                .eventai_unit(guid)
                .is_some_and(|unit| unit.map_id == map_id),
        }
    }

    fn eventai_in_zone_or_area(&self, unit: &EventAiUnit, zone_or_area_id: u32) -> bool {
        unit.zone_id == zone_or_area_id
            || self.eventai_areas.borrow().get(&unit.guid) == Some(&zone_or_area_id)
    }

    fn eventai_combat_action_ready(&self, guid: u64) -> bool {
        self.creatures
            .borrow()
            .get(&guid)
            .is_some_and(|creature| !creature.cannot_act)
            && !self.eventai_is_casting(guid)
            && !self.is_routing(guid)
    }

    fn eventai_rout_ends_ms(&self, creature_guid: u64) -> Option<u32> {
        self.fights
            .borrow()
            .iter()
            .any(|fight| fight.attacker == creature_guid)
            .then(|| self.rout_ends_ms(creature_guid))
    }

    fn eventai_broadcast(&self, id: u32, _speaker_guid: u64) -> Option<BroadcastLine> {
        self.eventai_text
            .borrow()
            .get(&id)
            .map(|(text, chat_type, language, emote)| BroadcastLine {
                text: text.clone(),
                chat_type: *chat_type,
                language: *language as u8,
                emote: *emote,
                sound: self
                    .eventai_text_sounds
                    .borrow()
                    .get(&id)
                    .copied()
                    .unwrap_or(0),
            })
    }

    fn eventai_summon_location(&self, id: u32) -> Option<SummonLocation> {
        self.eventai_summons
            .borrow()
            .get(&id)
            .map(|summon| SummonLocation {
                x: summon.at.x,
                y: summon.at.y,
                z: summon.at.z,
                orientation: summon.orientation,
                lifetime_ms: summon.lifetime_ms,
            })
    }

    fn eventai_summon_template_exists(&self, entry: u32) -> bool {
        self.eventai_templates.borrow().contains(&entry)
    }

    fn eventai_definition(&self, creature_guid: u64) -> EventAiDefinition {
        let Some(creature) = self.creatures.borrow().get(&creature_guid).copied() else {
            return EventAiDefinition::empty(creature_guid);
        };
        let mut definitions: Vec<EventAiDefinition> = self
            .eventai_definitions
            .borrow()
            .iter()
            .filter(|definition| {
                definition.subject == EventAiSubject::Entry(creature.entry)
                    || definition.subject == EventAiSubject::Guid(creature_guid)
            })
            .cloned()
            .collect();
        definitions.extend(self.legacy_eventai_definitions(creature.entry, creature_guid));
        EventAiDefinition::compose(creature_guid, definitions)
    }

    fn eventai_creature_state(&self, creature_guid: u64) -> CreatureState {
        self.eventai_creature_state
            .borrow()
            .get(&creature_guid)
            .copied()
            .unwrap_or_default()
    }

    fn adopt_eventai_revision(
        &mut self,
        creature_guid: u64,
        revision: DefinitionRevision,
    ) -> CreatureState {
        self.eventai_presentation
            .borrow_mut()
            .retain(|(guid, _, _, _)| *guid != creature_guid);
        self.eventai_rule_state.borrow_mut().remove(&creature_guid);
        self.eventai_lethal_floors
            .borrow_mut()
            .retain(|guid, owner| *guid != creature_guid || *owner == revision);
        self.eventai_movement.borrow_mut().remove(&creature_guid);
        let mut states = self.eventai_creature_state.borrow_mut();
        let state = states.entry(creature_guid).or_default();
        state.phase = 0;
        state.ranged_distance = 0.0;
        state.ranged_angle = 0.0;
        state.ranged_posture_active = false;
        state.definition_revision = revision;
        *state
    }

    fn set_eventai_phase(&mut self, creature_guid: u64, phase: u8) {
        self.eventai_creature_state
            .borrow_mut()
            .entry(creature_guid)
            .or_default()
            .phase = phase;
    }

    fn apply_eventai_presentation(
        &mut self,
        creature_guid: u64,
        lifecycle_id: u64,
        revision: DefinitionRevision,
        instruction: CreaturePresentationInstruction,
    ) -> bool {
        if self.corpses.borrow().contains(&creature_guid)
            || self.pet_owners.borrow().contains_key(&creature_guid)
            || !self.creatures.borrow().contains_key(&creature_guid)
        {
            return false;
        }
        self.eventai_presentation.borrow_mut().push((
            creature_guid,
            lifecycle_id,
            revision,
            instruction,
        ));
        true
    }

    fn eventai_rule_state(&self, creature_guid: u64, rule_id: u64) -> Option<RuleState> {
        self.eventai_rule_state
            .borrow()
            .get(&creature_guid)
            .and_then(|rules| rules.get(&rule_id))
            .copied()
    }

    fn put_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64, state: RuleState) {
        if !self.creatures.borrow().contains_key(&creature_guid) {
            return;
        }
        self.eventai_rule_state
            .borrow_mut()
            .entry(creature_guid)
            .or_default()
            .insert(rule_id, state);
    }

    fn delete_eventai_rule_state(&mut self, creature_guid: u64, rule_id: u64) {
        let mut state = self.eventai_rule_state.borrow_mut();
        if let Some(rules) = state.get_mut(&creature_guid) {
            rules.remove(&rule_id);
            if rules.is_empty() {
                state.remove(&creature_guid);
            }
        }
    }

    fn reap_eventai_rule_state(&mut self, creature_guid: u64, valid_rule_ids: &HashSet<u64>) {
        let mut state = self.eventai_rule_state.borrow_mut();
        if let Some(rules) = state.get_mut(&creature_guid) {
            rules.retain(|rule_id, _| valid_rule_ids.contains(rule_id));
            if rules.is_empty() {
                state.remove(&creature_guid);
            }
        }
    }

    fn eventai_roll(&self) -> u32 {
        self.rolls
            .borrow_mut()
            .pop_front()
            .expect("the scenario ran out of EventAI random rolls")
    }

    fn eventai_deliver_line(
        &mut self,
        speaker_guid: u64,
        target_guid: u64,
        chat_type: u8,
        _language: u8,
        message: String,
    ) -> bool {
        if self.corpses.borrow().contains(&speaker_guid) {
            return false;
        }
        let Some(message) = crate::chat::normalized_message(&message) else {
            return false;
        };
        self.eventai_speech
            .borrow_mut()
            .push((speaker_guid, chat_type, message));
        self.eventai_speech_targets
            .borrow_mut()
            .push((speaker_guid, target_guid));
        true
    }

    fn eventai_deliver_sound(&mut self, source_guid: u64, sound_id: u32, chat_type: u8) {
        self.eventai_sounds
            .borrow_mut()
            .push((source_guid, sound_id, chat_type));
    }

    fn eventai_deliver_emote(&mut self, source_guid: u64, emote_id: u32, target_guid: u64) -> bool {
        self.eventai_emotes
            .borrow_mut()
            .push((source_guid, 0, emote_id, target_guid));
        true
    }

    fn eventai_start_spell(
        &mut self,
        caster: &EventAiUnit,
        spell_id: u32,
        mode: SpellStartMode,
        target: SpellCastTarget,
        interrupt_previous: bool,
        admission: SpellCasterAdmission,
    ) -> bool {
        if self.not_ready.borrow().contains(&spell_id) {
            return false;
        }
        if interrupt_previous {
            self.casting.borrow_mut().remove(&caster.guid);
            self.channeling.borrow_mut().remove(&caster.guid);
        }
        self.eventai_spell_starts
            .borrow_mut()
            .push((mode, target, interrupt_previous, admission));
        let target_guid = match target {
            SpellCastTarget::Unit(guid) => guid,
            SpellCastTarget::None | SpellCastTarget::CasterArea => 0,
        };
        self.casts
            .borrow_mut()
            .push((caster.guid, spell_id, target_guid));
        true
    }

    fn eventai_credit_quest(
        &mut self,
        _request: EventAiQuestCredit,
        _credit_context: EventAiQuestCreditContext,
    ) -> QuestCreditOutcome {
        let outcome = self
            .eventai_quest_credit_outcomes
            .borrow_mut()
            .pop_front()
            .unwrap_or(QuestCreditOutcome::Applied);
        self.eventai_quest_credit_results.borrow_mut().push(outcome);
        outcome
    }

    fn stamp_eventai_rout(&mut self, creature_guid: u64, ends_ms: u32) {
        self.rout_clock.borrow_mut().insert(creature_guid, ends_ms);
    }

    fn set_eventai_ranged_posture(&mut self, creature_guid: u64, distance_yd: f32, angle_rad: f32) {
        let mut states = self.eventai_creature_state.borrow_mut();
        let state = states.entry(creature_guid).or_default();
        state.ranged_distance = distance_yd;
        state.ranged_angle = angle_rad;
        state.ranged_posture_active = true;
    }

    fn apply_eventai_movement(&mut self, creature_guid: u64, operation: MovementOperation) -> bool {
        let Some(creature) = self.creatures.borrow().get(&creature_guid).copied() else {
            return false;
        };
        if matches!(operation, MovementOperation::SetCombatMovement(_))
            && self.eventai_is_casting(creature_guid)
        {
            return false;
        }
        match operation {
            MovementOperation::SetRangedMode(ranged) => {
                if !self.authored_combat(creature_guid).main_spell_posture {
                    return false;
                }
                let distance = if ranged.mode == RangedMode::None {
                    0.0
                } else {
                    ranged.distance_yd as f32
                };
                self.set_eventai_ranged_posture(creature_guid, distance, 0.0);
                return true;
            }
            MovementOperation::Evade(evade) => {
                self.fights
                    .borrow_mut()
                    .retain(|fight| fight.attacker != creature_guid);
                self.engaged.borrow_mut().remove(&creature_guid);
                if !evade.combat_only {
                    self.eventai_returning_home
                        .borrow_mut()
                        .insert(creature_guid);
                }
                return true;
            }
            _ => {}
        }

        let current = self.movement_intent(creature_guid).unwrap_or_default();
        let mut intent = current;
        intent.creature_entry = creature.entry;
        intent.map_id = creature.map_id;
        intent.instance_id = creature.instance_id;
        match operation {
            MovementOperation::ReplaceIdle(idle) => {
                if let IdleMovementIntent::Patrol(patrol) = idle {
                    let exists = if patrol.path_id == 0 {
                        self.routes.borrow().contains_key(&creature_guid)
                    } else {
                        self.eventai_paths.borrow().contains_key(&(
                            creature.entry,
                            creature.map_id,
                            patrol.path_id,
                        ))
                    };
                    if !exists {
                        return false;
                    }
                }
                intent.idle = Some(idle);
                intent.anchor = matches!(idle, IdleMovementIntent::RandomAroundCurrentPosition(_))
                    .then_some(creature.at);
                intent.patrol_paused = false;
            }
            MovementOperation::SetPatrolPaused(pause) => {
                if !matches!(intent.idle, Some(IdleMovementIntent::Patrol(_))) {
                    return false;
                }
                intent.patrol_paused = pause.paused;
            }
            MovementOperation::SetCombatMovement(switch) => {
                if intent.combat_movement.unwrap_or(true) == switch.enabled {
                    return false;
                }
                intent.combat_movement = Some(switch.enabled);
            }
            MovementOperation::SetWalking(mode) => intent.walking = Some(mode),
            MovementOperation::SetImmobilized(immobilized) => {
                intent.immobilized = immobilized.enabled;
                intent.immobilized_combat_only = immobilized.enabled && immobilized.combat_only;
            }
            MovementOperation::SetFollowMovement(switch) => {
                intent.follow_movement = Some(switch.enabled);
            }
            MovementOperation::Face(target_guid) => {
                let Some(target) = self.creatures.borrow().get(&target_guid).copied() else {
                    return false;
                };
                if target.map_id != creature.map_id || target.instance_id != creature.instance_id {
                    return false;
                }
                intent.facing = Some(ScenarioFacing::Target(target_guid));
            }
            MovementOperation::ResetFacing => intent.facing = Some(ScenarioFacing::Reset),
            MovementOperation::SetRangedMode(_) | MovementOperation::Evade(_) => unreachable!(),
        }
        self.eventai_movement
            .borrow_mut()
            .insert(creature_guid, intent);
        true
    }

    fn eventai_engage_assist(&mut self, creature_guid: u64, victim_guid: u64) {
        if !self
            .fights
            .borrow()
            .iter()
            .any(|fight| fight.attacker == creature_guid)
        {
            EngageSink::engage(self, creature_guid, victim_guid, Pull::Assisted);
        }
    }

    fn eventai_claim_summon_sequence(&mut self, _lifetime_ms: u32) -> u64 {
        let sequence = self.eventai_next_summon.get() + 1;
        self.eventai_next_summon.set(sequence);
        sequence
    }

    fn eventai_release_summon_sequence(&mut self, _sequence: u64) {}

    fn eventai_place_summon(
        &mut self,
        _sequence: u64,
        guid: u64,
        entry: u32,
        location: &SummonLocation,
        summoner: &EventAiUnit,
    ) {
        let at = Point {
            x: location.x,
            y: location.y,
            z: location.z,
        };
        let (grid_x, grid_y) = spatial::grid_cell(at.x, at.y);
        self.creatures.borrow_mut().insert(
            guid,
            XCreature {
                entry,
                at,
                grid: (grid_x, grid_y),
                cell: spatial::grid_cell_id(grid_x, grid_y),
                last_move_ms: 0,
                orientation: location.orientation,
                leg_ends_ms: 0,
                wp_target: 0,
                health: 100,
                max_health: 100,
                power: 0,
                max_power: 0,
                level: 10,
                faction_template: summoner.faction_template,
                map_id: summoner.map_id,
                instance_id: summoner.instance_id,
                aggro_range: Some(0),
                detect_range_mod: 0.0,
                would_rout: false,
                cannot_act: false,
            },
        );
        self.eventai_summon_expiry.borrow_mut().insert(
            guid,
            ScenarioSummonExpiry {
                lifetime_ms: location.lifetime_ms,
                remaining_ms: location.lifetime_ms,
                last_checked_ms: self.now_micros.get() / 1_000,
                summoner_guid: summoner.guid,
            },
        );
        eventai::evaluate(
            self,
            EventAiRequest::Edge(EventContext {
                spawner_guid: Some(summoner.guid),
                ..EventContext::empty(EventKind::OnSpawn, guid, self.now_micros.get() / 1_000)
            }),
        );
        eventai::evaluate(
            self,
            EventAiRequest::Edge(EventContext {
                invoker_guid: Some(guid),
                creature_entry: Some(entry),
                ..EventContext::empty(
                    EventKind::OnSummoned,
                    summoner.guid,
                    self.now_micros.get() / 1_000,
                )
            }),
        );
    }

    fn eventai_engage_summon(&mut self, summon_guid: u64, target_guid: u64) {
        EngageSink::engage(self, summon_guid, target_guid, Pull::Assisted);
    }

    fn eventai_start_attack(&mut self, creature_guid: u64, target_guid: u64) -> bool {
        if let Some(fight) = self
            .fights
            .borrow_mut()
            .iter_mut()
            .find(|fight| fight.attacker == creature_guid)
        {
            fight.victim = target_guid;
            return true;
        }
        EngageSink::engage(self, creature_guid, target_guid, Pull::Noticed);
        true
    }

    fn eventai_remove_aura(&mut self, target_guid: u64, spell_id: u32) -> bool {
        if self.eventai_unit(target_guid).is_none() {
            return false;
        }
        self.auras.borrow_mut().remove(&(target_guid, spell_id));
        true
    }

    fn eventai_force_despawn(&mut self, creature_guid: u64, delay_ms: u32) -> bool {
        if !self.creatures.borrow().contains_key(&creature_guid) {
            return false;
        }
        self.eventai_forced_despawns
            .borrow_mut()
            .push((creature_guid, delay_ms));
        if delay_ms == 0 {
            self.clear_eventai_summon(creature_guid);
            self.creatures.borrow_mut().remove(&creature_guid);
        }
        true
    }

    fn eventai_throw_ai_event(
        &mut self,
        source_guid: u64,
        invoker_guid: u64,
        kind: eventai::AiEventKind,
        radius_yd: u32,
    ) -> bool {
        if self.eventai_unit(source_guid).is_none() || self.eventai_unit(invoker_guid).is_none() {
            return false;
        }
        self.eventai_ai_events
            .borrow_mut()
            .push((source_guid, invoker_guid, kind, radius_yd));
        true
    }

    fn eventai_set_auto_attack(&mut self, creature_guid: u64, enabled: bool) -> bool {
        if self.eventai_unit(creature_guid).is_none() {
            return false;
        }
        self.eventai_auto_attacks
            .borrow_mut()
            .insert(creature_guid, enabled);
        true
    }
    fn eventai_set_sheath(&mut self, creature_guid: u64, state: u8) -> bool {
        if self.eventai_unit(creature_guid).is_none() {
            return false;
        }
        self.eventai_sheaths
            .borrow_mut()
            .insert(creature_guid, state);
        true
    }
    fn eventai_combat_candidates(&self, creature_guid: u64) -> Vec<EventAiUnit> {
        let Some(creature) = self.eventai_unit(creature_guid) else {
            return Vec::new();
        };
        self.eventai_units_near(&creature, f32::MAX)
            .into_iter()
            .filter(|target| {
                target.is_player
                    && !target.dead
                    && target.map_id == creature.map_id
                    && target.instance_id == creature.instance_id
                    && self.eventai_factions_hostile(
                        creature.faction_template,
                        target.faction_template,
                    )
            })
            .collect()
    }
    fn eventai_engage_without_threat(&mut self, creature_guid: u64, target_guid: u64) {
        self.threat
            .borrow_mut()
            .entry((creature_guid, target_guid))
            .or_insert(0);
    }
    fn eventai_set_stand_state(&mut self, creature_guid: u64, stand_state: u8) -> bool {
        if self.eventai_unit(creature_guid).is_none() {
            return false;
        }
        self.eventai_stand_states
            .borrow_mut()
            .insert(creature_guid, stand_state);
        true
    }

    fn eventai_set_react_state(&mut self, creature_guid: u64, state: CreatureReactState) -> bool {
        if self.eventai_unit(creature_guid).is_none() {
            return false;
        }
        self.eventai_react_states
            .borrow_mut()
            .insert(creature_guid, state);
        true
    }

    fn eventai_remove_guardians(&mut self, summoner_guid: u64, creature_entry: u32) -> bool {
        if self.eventai_unit(summoner_guid).is_none() {
            return false;
        }
        let mut guardians = self
            .pet_owners
            .borrow()
            .iter()
            .filter(|(_, owner)| **owner == summoner_guid)
            .filter_map(|(guid, _)| {
                self.creatures
                    .borrow()
                    .get(guid)
                    .filter(|creature| creature_entry == 0 || creature.entry == creature_entry)
                    .map(|_| *guid)
            })
            .collect::<Vec<_>>();
        guardians.sort_unstable();
        if creature_entry != 0 {
            guardians.truncate(1);
        }
        for guardian in guardians {
            self.pet_owners.borrow_mut().remove(&guardian);
            self.creatures.borrow_mut().remove(&guardian);
        }
        true
    }

    fn eventai_set_lethal_damage_floor(
        &mut self,
        creature_guid: u64,
        revision: DefinitionRevision,
        enabled: bool,
    ) -> bool {
        if !self.creatures.borrow().contains_key(&creature_guid)
            || self.corpses.borrow().contains(&creature_guid)
        {
            return false;
        }
        if enabled {
            self.eventai_lethal_floors
                .borrow_mut()
                .insert(creature_guid, revision);
        } else {
            self.eventai_lethal_floors
                .borrow_mut()
                .remove(&creature_guid);
        }
        true
    }

    fn eventai_force_death(&mut self, creature_guid: u64) -> bool {
        if !self.creatures.borrow().contains_key(&creature_guid)
            || self.corpses.borrow().contains(&creature_guid)
        {
            return false;
        }
        self.eventai_lethal_floors
            .borrow_mut()
            .remove(&creature_guid);
        self.corpses.borrow_mut().insert(creature_guid);
        self.eventai_forced_deaths.borrow_mut().push(creature_guid);
        true
    }

    fn eventai_notify_encounter(
        &mut self,
        source_guid: u64,
        notification: NotifyEncounterInstruction,
    ) -> bool {
        let Some(source) = self.eventai_unit(source_guid) else {
            return false;
        };
        if source.instance_id == 0 || source.map_id != notification.binding.map_id() {
            return false;
        }
        self.eventai_encounter_notifications
            .borrow_mut()
            .push((source_guid, notification));
        true
    }

    fn eventai_start_relay(
        &mut self,
        relay_id: u32,
        source_guid: u64,
        selected_guid: u64,
        random_state: u64,
        _catalogue_version: u64,
    ) -> bool {
        self.eventai_relay_starts.borrow_mut().push((
            relay_id,
            source_guid,
            selected_guid,
            random_state,
        ));
        true
    }
}

impl IdleSink for Scenario {
    fn idle_creatures(&self, active: &HashSet<u64>) -> Vec<IdleCreature> {
        let creatures = self.creatures.borrow();
        let routes = self.routes.borrow();
        active
            .iter()
            .filter_map(|guid| creatures.get(guid).map(|c| (guid, c)))
            .map(|(guid, c)| IdleCreature {
                guid: *guid,
                at: c.at,
                leg_ends_ms: c.leg_ends_ms,
                wp_target: c.wp_target,
                patrols: match self.movement_intent(*guid).and_then(|intent| intent.idle) {
                    Some(IdleMovementIntent::Patrol(_)) => true,
                    Some(
                        IdleMovementIntent::Stationary
                        | IdleMovementIntent::RandomAroundHomePosition(_)
                        | IdleMovementIntent::RandomAroundCurrentPosition(_),
                    ) => false,
                    None => routes.contains_key(guid),
                },
            })
            .collect()
    }
    fn route_of(&self, guid: u64) -> Vec<Waypoint> {
        if let Some(IdleMovementIntent::Patrol(patrol)) =
            self.movement_intent(guid).and_then(|intent| intent.idle)
        {
            if patrol.path_id != 0 {
                let creature = self.creatures.borrow()[&guid];
                return self
                    .eventai_paths
                    .borrow()
                    .get(&(creature.entry, creature.map_id, patrol.path_id))
                    .cloned()
                    .unwrap_or_default();
            }
        }
        self.routes.borrow().get(&guid).cloned().unwrap_or_default()
    }
    fn home_of(&self, guid: u64) -> Option<Home> {
        if let Some(intent) = self.movement_intent(guid) {
            if matches!(
                intent.idle,
                Some(IdleMovementIntent::RandomAroundCurrentPosition(_))
            ) {
                return intent.anchor.map(|at| Home { at, wanders: true });
            }
            if matches!(
                intent.idle,
                Some(IdleMovementIntent::RandomAroundHomePosition(_))
            ) {
                return self.homes.borrow().get(&guid).copied().map(|home| Home {
                    wanders: true,
                    ..home
                });
            }
        }
        self.homes.borrow().get(&guid).copied()
    }
    fn return_home_of(&self, guid: u64) -> Option<Home> {
        self.homes.borrow().get(&guid).copied()
    }
    fn patrol_paused(&self, guid: u64) -> bool {
        self.movement_intent(guid)
            .is_some_and(|intent| intent.patrol_paused)
    }
    fn idle_stationary(&self, guid: u64) -> bool {
        self.movement_intent(guid)
            .is_some_and(|intent| intent.idle == Some(IdleMovementIntent::Stationary))
    }
    fn returning_home(&self, guid: u64) -> bool {
        self.eventai_returning_home.borrow().contains(&guid)
    }
    fn follow_target_at(&self, guid: u64) -> Option<Point> {
        if self
            .movement_intent(guid)
            .and_then(|intent| intent.follow_movement)
            != Some(true)
        {
            return None;
        }
        let creature = self.creatures.borrow().get(&guid).copied()?;
        let target_guid = self.eventai_spawner_guid(guid)?;
        self.creatures
            .borrow()
            .get(&target_guid)
            .filter(|target| {
                target.map_id == creature.map_id && target.instance_id == creature.instance_id
            })
            .map(|target| target.at)
    }
    fn pending_facing(&self, guid: u64) -> Option<f32> {
        let creature = self.creatures.borrow().get(&guid).copied()?;
        match self.movement_intent(guid).and_then(|intent| intent.facing) {
            Some(ScenarioFacing::Reset) => Some(0.0),
            Some(ScenarioFacing::Target(target_guid)) => self
                .creatures
                .borrow()
                .get(&target_guid)
                .filter(|target| {
                    target.map_id == creature.map_id && target.instance_id == creature.instance_id
                })
                .map(|target| (target.at.y - creature.at.y).atan2(target.at.x - creature.at.x)),
            None => None,
        }
    }
    fn clear_pending_facing(&mut self, guid: u64) {
        if self.movement_intent(guid).is_some() {
            let mut movement = self.eventai_movement.borrow_mut();
            let intent = movement.get_mut(&guid).unwrap();
            intent.facing = None;
        }
    }
    fn idle_gait(&self, guid: u64, default: Gait) -> Gait {
        match self.movement_intent(guid).and_then(|intent| intent.walking) {
            Some(WalkingMode::RunByDefault) if default == Gait::Walk => Gait::Run,
            Some(WalkingMode::WalkByDefault) if default == Gait::Run => Gait::Walk,
            _ => default,
        }
    }
    fn wander_radius(&self, guid: u64) -> f32 {
        self.movement_intent(guid)
            .and_then(|intent| intent.idle)
            .and_then(|idle| match idle {
                IdleMovementIntent::RandomAroundHomePosition(random)
                | IdleMovementIntent::RandomAroundCurrentPosition(random) => {
                    Some(random.radius_yd as f32)
                }
                _ => None,
            })
            .unwrap_or(WANDER_RADIUS)
    }
    fn engaged(&self, guid: u64) -> bool {
        self.engaged.borrow().contains(&guid)
            || self
                .fights
                .borrow()
                .iter()
                .any(|f| f.attacker == guid || f.victim == guid)
    }
    fn speed_of(&self, _guid: u64, gait: Gait) -> f32 {
        match gait {
            Gait::Walk => lyracore_shared::constants::speeds::WALK,
            Gait::Run => lyracore_shared::constants::speeds::RUN,
        }
    }
    fn navigate(&self, guid: u64, to: (f32, f32), max_step: f32, stop_dist: f32) -> (f32, f32) {
        let from = self.creatures.borrow()[&guid].at;
        match self.detours.borrow().get(&guid) {
            Some(corner) => chase_step(from.x, from.y, corner.0, corner.1, max_step, 0.0),
            None => chase_step(from.x, from.y, to.0, to.1, max_step, stop_dist),
        }
    }
    fn roll(&self) -> u32 {
        self.rolls
            .borrow_mut()
            .pop_front()
            .expect("the scenario ran out of random rolls")
    }
    fn aim_at_waypoint(&mut self, guid: u64, waypoint_id: u64) {
        if let Some(c) = self.creatures.borrow_mut().get_mut(&guid) {
            c.wp_target = waypoint_id;
        }
    }
    fn reached_home(&mut self, guid: u64) {
        if !self.eventai_returning_home.borrow_mut().remove(&guid) {
            return;
        }
        eventai::evaluate(
            self,
            EventAiRequest::Edge(EventContext::empty(
                EventKind::OnReachedHome,
                guid,
                self.now_micros.get() / 1_000,
            )),
        );
    }
    fn commit_leg(&mut self, guid: u64, leg: Leg) {
        let Some(from) = self.creatures.borrow().get(&guid).copied() else {
            return;
        };
        let now_ms = (self.now_micros.get() / 1000) as u32;
        let dest = Point {
            x: leg.to.0,
            y: leg.to.1,
            z: self.ground.get().unwrap_or(leg.z_fallback),
        };
        if !finite_point(dest.x, dest.y, dest.z) {
            return;
        }
        let dur_ms = leg.dur_ms.max(1);
        self.record(MoveEffect {
            guid,
            start: from.at,
            dest,
            dur_ms,
            spline_id: self.next_spline_id(guid),
            run: leg.gait == Gait::Run,
            map_id: MAP,
            instance_id: INSTANCE,
            grid: from.grid,
            cell: from.cell,
            facing: false,
            facing_angle: 0.0,
        });
        let mut legs = self.legs.borrow_mut();
        legs.retain(|l| l.guid != guid);
        legs.push(LegInFlight {
            guid,
            start: from.at,
            dest,
            started_micros: self.now_micros.get(),
            dur_ms,
            map_id: MAP,
            instance_id: INSTANCE,
            back_off_from: 0,
            mover_gone: false,
            waypoints: Vec::new(),
        });
        let mut creatures = self.creatures.borrow_mut();
        let c = creatures.get_mut(&guid).unwrap();
        c.last_move_ms = now_ms;
        if (dest.x, dest.y) != (from.at.x, from.at.y) {
            c.orientation = (dest.y - from.at.y).atan2(dest.x - from.at.x);
        }
        if leg.hold_until_landed {
            c.leg_ends_ms = now_ms + leg.dur_ms;
        }
    }
}

impl EngageSink for Scenario {
    fn players(&self) -> Vec<AggroTarget> {
        self.players.borrow().clone()
    }
    fn sensing_creatures(&self, active: &HashSet<u64>) -> Vec<Sensor> {
        let creatures = self.creatures.borrow();
        let fights = self.fights.borrow();
        active
            .iter()
            .filter_map(|guid| creatures.get(guid).map(|c| (*guid, c)))
            .filter(|(guid, _)| !fights.iter().any(|f| f.attacker == *guid))
            .map(|(guid, c)| Sensor {
                guid,
                at: c.at,
                level: c.level,
                faction_template: c.faction_template,
                map_id: c.map_id,
                instance_id: c.instance_id,
                aggro_range: c.aggro_range,
                detect_range_mod: c.detect_range_mod,
                would_rout: c.would_rout,
                cannot_act: c.cannot_act || self.cc(guid).0,
                react_state: self
                    .eventai_react_states
                    .borrow()
                    .get(&guid)
                    .copied()
                    .unwrap_or(CreatureReactState::Aggressive),
            })
            .collect()
    }
    fn hostile(&self, faction_template: u32, other: u32) -> bool {
        self.at_war.borrow().contains(&(faction_template, other))
    }
    fn line_of_sight(&self, looker: u64, at: Point) -> bool {
        let seen = self
            .creatures
            .borrow()
            .iter()
            .find(|(_, c)| (c.at.x, c.at.y) == (at.x, at.y))
            .map(|(guid, _)| *guid)
            .or_else(|| {
                self.players
                    .borrow()
                    .iter()
                    .find(|p| (p.at.x, p.at.y) == (at.x, at.y))
                    .map(|p| p.guid)
            });
        seen.is_none_or(|seen| !self.blind.borrow().contains(&(looker, seen)))
    }
    fn engage(&mut self, creature: u64, victim: u64, pull: Pull) {
        self.pulls.borrow_mut().push((creature, victim, pull));
        let instance_id = self.creatures.borrow()[&creature].instance_id;
        self.fights.borrow_mut().push(Engagement {
            attacker: creature,
            victim,
            instance_id,
            player_never_swung: false,
        });
        let request = EventAiRequest::Edge(EventContext {
            invoker_guid: Some(victim),
            beneficiary_guid: Some(victim),
            current_target_guid: Some(victim),
            assisted: pull == Pull::Assisted,
            engaged: true,
            ..EventContext::empty(EventKind::OnAggro, creature, self.now_micros.get() / 1000)
        });
        eventai::evaluate(self, request);
    }
    fn engagements(&self) -> Vec<Engagement> {
        let creatures = self.creatures.borrow();
        let mut engagements: Vec<Engagement> = self
            .fights
            .borrow()
            .iter()
            .map(|f| Engagement {
                instance_id: creatures
                    .get(&f.attacker)
                    .map_or(f.instance_id, |c| c.instance_id),
                ..*f
            })
            .collect();
        let threats = self.threat.borrow();
        let members = engagements
            .iter()
            .flat_map(|engagement| {
                threats
                    .keys()
                    .filter(|(creature, _)| *creature == engagement.attacker)
                    .map(|(_, target)| Engagement {
                        victim: *target,
                        player_never_swung: false,
                        ..*engagement
                    })
            })
            .collect::<Vec<_>>();
        engagements.extend(members);
        engagements
    }
    fn enter_combat(&mut self, guid: u64) {
        self.flagged.borrow_mut().push(guid);
        let now_ms = self.now_micros.get() / 1000;
        self.combat_flags
            .borrow_mut()
            .insert(guid, now_ms + crate::combat::COMBAT_DROP_MS);
    }
    fn flagged_in_combat(&self, candidates: &[u64]) -> Vec<Combatant> {
        let flags = self.combat_flags.borrow();
        candidates
            .iter()
            .filter_map(|guid| {
                flags.get(guid).map(|until_ms| Combatant {
                    guid: *guid,
                    combat_until_ms: *until_ms,
                })
            })
            .collect()
    }
    fn leave_combat(&mut self, guid: u64) {
        let definition = self.eventai_definition(guid);
        let rule_ids: HashSet<u64> = definition
            .rules
            .iter()
            .filter(|rule| {
                !matches!(
                    rule.event,
                    EventCondition::OnDeath(_)
                        | EventCondition::OnSpawn(_)
                        | EventCondition::TimedGeneric(_)
                )
            })
            .map(|rule| rule.source_rule_id)
            .collect();
        let lifecycle_rule_ids: HashSet<u64> = definition
            .rules
            .iter()
            .filter(|rule| {
                matches!(
                    rule.event,
                    EventCondition::OnDeath(_)
                        | EventCondition::OnSpawn(_)
                        | EventCondition::TimedGeneric(_)
                )
            })
            .map(|rule| rule.source_rule_id)
            .collect();
        let engagement_id = self
            .eventai_creature_state
            .borrow_mut()
            .get_mut(&guid)
            .map(|state| {
                state.engagement_id = state.engagement_id.saturating_add(1);
                state.phase = 0;
                state.ranged_distance = 0.0;
                state.ranged_angle = 0.0;
                state.ranged_posture_active = false;
                state.engagement_id
            });
        if let Some(state) = self.eventai_rule_state.borrow_mut().get_mut(&guid) {
            state.retain(|rule_id, _| !rule_ids.contains(rule_id));
            if let Some(engagement_id) = engagement_id {
                for (rule_id, state) in state {
                    if lifecycle_rule_ids.contains(rule_id) {
                        state.engagement_id = engagement_id;
                    }
                }
            }
        }
        self.unflagged.borrow_mut().push(guid);
        self.combat_flags.borrow_mut().remove(&guid);
        if let Some(intent) = self.eventai_movement.borrow_mut().get_mut(&guid) {
            if intent.immobilized_combat_only {
                intent.immobilized = false;
                intent.immobilized_combat_only = false;
            }
        }
    }
}

const REGEN_TICK: u32 = 5;
const COMBAT_REGEN_TICK: u32 = 1;

impl RegenSink for Scenario {
    fn recovering(&self, flagged: &[u64]) -> Vec<Recovering> {
        let fights = self.fights.borrow();
        let corpses = self.corpses.borrow();
        let mut units: Vec<Recovering> = self
            .creatures
            .borrow()
            .iter()
            .filter(|(guid, c)| {
                !corpses.contains(*guid) && (c.health < c.max_health || c.max_power > 0)
            })
            .map(|(guid, c)| Recovering {
                guid: *guid,
                health: c.health,
                max_health: c.max_health,
                power: c.power,
                max_power: c.max_power,
                in_combat: flagged.contains(guid)
                    || fights
                        .iter()
                        .any(|f| f.attacker == *guid || f.victim == *guid),
            })
            .collect();
        units.sort_unstable_by_key(|u| u.guid);
        units
    }
    fn healed_to(&self, u: &Recovering) -> u32 {
        (u.health + REGEN_TICK).min(u.max_health)
    }
    fn combat_healed_to(&self, u: &Recovering) -> Option<u32> {
        self.combat_regen
            .borrow()
            .contains(&u.guid)
            .then(|| (u.health + COMBAT_REGEN_TICK).min(u.max_health))
    }
    fn powered_to(&self, u: &Recovering) -> u32 {
        (u.power + REGEN_TICK).min(u.max_power)
    }
    fn restore(&mut self, guid: u64, health: Option<u32>, power: Option<u32>) {
        let mut creatures = self.creatures.borrow_mut();
        let Some(c) = creatures.get_mut(&guid) else {
            return;
        };
        c.health = health.unwrap_or(c.health);
        c.power = power.unwrap_or(c.power);
    }
}

impl CastSink for Scenario {
    fn casters(&self, scope: &TickScope) -> Vec<Caster> {
        let creatures = self.creatures.borrow();
        self.fights
            .borrow()
            .iter()
            .filter_map(|f| {
                let c = creatures.get(&f.attacker)?;
                scope.covers(c.instance_id).then_some(Caster {
                    guid: f.attacker,
                    victim: f.victim,
                    victim_at: self.unit(f.victim).map(|(at, _)| at),
                    level: c.level as u8,
                    health: c.health,
                    max_health: c.max_health,
                    cannot_act: c.cannot_act || self.cc(f.attacker).0,
                    casting: self.eventai_is_casting(f.attacker),
                })
            })
            .collect()
    }
    fn rotation_of(&self, guid: u64) -> Vec<SpellOption> {
        if self.authored_combat(guid).casting {
            return Vec::new();
        }
        self.rotations
            .borrow()
            .get(&guid)
            .cloned()
            .unwrap_or_default()
    }
    fn creature_spell_list_of(&self, guid: u64) -> Vec<SpellOption> {
        self.creature_spell_lists
            .borrow()
            .get(&guid)
            .cloned()
            .unwrap_or_default()
    }
    fn lone_spell(&self, guid: u64) -> Option<u32> {
        if self.authored_combat(guid).casting {
            return None;
        }
        self.lone_spells.borrow().get(&guid).copied()
    }
    fn carries(&self, guid: u64, spell_id: u32) -> bool {
        self.auras.borrow().contains(&(guid, spell_id))
    }
    fn begin_cast(&mut self, caster: &Caster, spell_id: u32, target: u64) -> bool {
        if self.not_ready.borrow().contains(&spell_id) {
            return false;
        }
        self.casts
            .borrow_mut()
            .push((caster.guid, spell_id, target));
        self.auras.borrow_mut().insert((target, spell_id));
        true
    }
}

impl ThreatSink for Scenario {
    fn fighters(&self, scope: &TickScope) -> Vec<Fighter> {
        let creatures = self.creatures.borrow();
        self.fights
            .borrow()
            .iter()
            .filter_map(|f| {
                let c = creatures.get(&f.attacker)?;
                scope.covers(c.instance_id).then_some(Fighter {
                    guid: f.attacker,
                    victim: f.victim,
                })
            })
            .collect()
    }
    fn taunted_onto(&self, guid: u64) -> Option<u64> {
        self.taunts.borrow().get(&guid).copied()
    }
    fn top_threat(&self, guid: u64) -> Option<u64> {
        crate::threat::rank(
            self.threat
                .borrow()
                .iter()
                .filter(|((creature, _), _)| *creature == guid)
                .map(|((_, source), threat)| (*source, *threat))
                .collect(),
        )
        .first()
        .map(|&(source, _)| source)
    }
    fn threat_on(&self, creature: u64, source: u64) -> i64 {
        self.threat
            .borrow()
            .get(&(creature, source))
            .copied()
            .unwrap_or(0)
    }
    fn retarget(&mut self, creature: u64, victim: u64) {
        for f in self.fights.borrow_mut().iter_mut() {
            if f.attacker == creature {
                f.victim = victim;
            }
        }
    }
}

impl PursuitSink for Scenario {
    fn pursuits(&self, scope: &TickScope) -> Vec<Pursuit> {
        let creatures = self.creatures.borrow();
        let legs = self.legs.borrow();
        self.fights
            .borrow()
            .iter()
            .filter_map(|f| {
                let c = creatures.get(&f.attacker)?;
                let (victim_at, victim_movement_flags) = self.unit(f.victim)?;
                scope.covers(c.instance_id).then_some(Pursuit {
                    guid: f.attacker,
                    victim: f.victim,
                    at: c.at,
                    orientation: c.orientation,
                    victim_at,
                    victim_orientation: self
                        .creatures
                        .borrow()
                        .get(&f.victim)
                        .map_or(0.0, |victim| victim.orientation),
                    victim_movement_flags,
                    victim_is_character: self.players.borrow().iter().any(|p| p.guid == f.victim),
                    last_move_ms: c.last_move_ms,
                    routing: self.is_routing(f.attacker),
                    leg: legs.iter().find(|l| l.guid == f.attacker).cloned(),
                })
            })
            .collect()
    }
    fn authored_ranged_posture(&self, guid: u64) -> Option<(f32, f32)> {
        self.eventai_creature_state
            .borrow()
            .get(&guid)
            .filter(|state| state.ranged_posture_active)
            .map(|state| (state.ranged_distance, state.ranged_angle))
    }
    fn combat_movement_enabled(&self, guid: u64) -> bool {
        self.movement_intent(guid)
            .and_then(|intent| intent.combat_movement)
            .unwrap_or(true)
    }
    fn chase_gait(&self, guid: u64) -> Gait {
        match self.movement_intent(guid).and_then(|intent| intent.walking) {
            Some(WalkingMode::WalkWhileChasing) => Gait::Walk,
            _ => Gait::Run,
        }
    }
    fn target_not_reachable(&mut self, guid: u64) {
        let current_target_guid = self
            .fights
            .borrow()
            .iter()
            .find(|fight| fight.attacker == guid)
            .map(|fight| fight.victim);
        eventai::evaluate(
            self,
            EventAiRequest::Edge(EventContext {
                current_target_guid,
                engaged: current_target_guid.is_some(),
                ..EventContext::empty(
                    EventKind::TargetNotReachable,
                    guid,
                    self.now_micros.get() / 1_000,
                )
            }),
        );
    }
    fn caster_hold_range(&self, guid: u64) -> f32 {
        if self.authored_combat(guid).casting {
            return 0.0;
        }
        self.hold_ranges.borrow().get(&guid).copied().unwrap_or(0.0)
    }
    fn casting(&self, guid: u64) -> bool {
        self.eventai_is_casting(guid)
    }
    fn commit_back_off(&mut self, guid: u64, victim: u64, to: (f32, f32), dur_ms: u32) {
        let z_fallback = self.at(guid).at.z;
        self.commit_leg(
            guid,
            Leg {
                to,
                z_fallback,
                dur_ms,
                gait: Gait::Walk,
                hold_until_landed: false,
            },
        );
        for committed in self.legs.borrow_mut().iter_mut() {
            if committed.guid == guid {
                committed.back_off_from = victim;
            }
        }
    }
    fn face(&mut self, guid: u64, at: Point, orientation: f32) {
        let Some(c) = self.place(guid, at, None, Some(orientation)) else {
            return;
        };
        self.record(MoveEffect {
            guid,
            start: at,
            dest: at,
            dur_ms: 0,
            spline_id: self.next_spline_id(guid),
            run: false,
            map_id: c.map_id,
            instance_id: c.instance_id,
            grid: c.grid,
            cell: c.cell,
            facing: true,
            facing_angle: orientation,
        });
        self.park_leg(guid, at, c.map_id, c.instance_id);
    }
}

impl RoutSink for Scenario {
    fn routers(&self, scope: &TickScope) -> Vec<Router> {
        let creatures = self.creatures.borrow();
        let legs = self.legs.borrow();
        self.fights
            .borrow()
            .iter()
            .filter_map(|f| {
                let c = creatures.get(&f.attacker)?;
                scope.covers(c.instance_id).then_some(Router {
                    guid: f.attacker,
                    at: c.at,
                    victim: f.victim,
                    victim_at: self.unit(f.victim).map(|(at, _)| at),
                    health: c.health,
                    max_health: c.max_health,
                    eligible: c.would_rout && !self.authored_combat(f.attacker).flee,
                    rout_ends_ms: self.rout_ends_ms(f.attacker),
                    routing: self.is_routing(f.attacker),
                    committed: legs.iter().any(|l| l.guid == f.attacker),
                })
            })
            .collect()
    }
    fn start_rout(&mut self, guid: u64, ends_ms: u32) {
        self.rout_clock.borrow_mut().insert(guid, ends_ms);
    }
}

impl FearSink for Scenario {
    fn panicked(&self, scope: &TickScope) -> Vec<Panicked> {
        let creatures = self.creatures.borrow();
        let fears = self.fears.borrow();
        fears
            .iter()
            .filter_map(|(guid, _)| {
                let c = creatures.get(guid)?;
                let (_, caster) = fears.iter().find(|(unit, _)| unit == guid)?;
                let source = self.unit(*caster).map(|(at, _)| at).or_else(|| {
                    let victim = self
                        .fights
                        .borrow()
                        .iter()
                        .find(|f| f.attacker == *guid)
                        .map(|f| f.victim)?;
                    self.unit(victim).map(|(at, _)| at)
                });
                scope.covers(c.instance_id).then_some(Panicked {
                    guid: *guid,
                    at: c.at,
                    source_at: source,
                    frozen: self.cc(*guid).1,
                })
            })
            .collect()
    }
}

impl PetSink for Scenario {
    fn pets(&self, scope: &TickScope, candidates: &[u64]) -> Vec<Pet> {
        let creatures = self.creatures.borrow();
        let owners = self.pet_owners.borrow();
        let bars = self.pet_bars.borrow();
        candidates
            .iter()
            .filter_map(|guid| {
                let c = creatures.get(guid)?;
                let owner_guid = *owners.get(guid)?;
                let (command, react) = bars
                    .get(&owner_guid)
                    .copied()
                    .unwrap_or((PetOrder::Follow, PetReact::Defensive));
                scope.covers(c.instance_id).then_some(Pet {
                    guid: *guid,
                    at: c.at,
                    map_id: c.map_id,
                    instance_id: c.instance_id,
                    owner: self.pet_owner(owner_guid),
                    dead: self.corpses.borrow().contains(guid),
                    suppressed: self.cc(*guid).2,
                    command,
                    react,
                    fighting: self.fights.borrow().iter().any(|f| f.attacker == *guid),
                })
            })
            .collect()
    }
    fn pet_may_attack(&self, pet: &Pet, target: u64) -> bool {
        let Some(owner) = pet.owner else {
            return false;
        };
        if target == 0 || target == pet.guid || target == owner.guid {
            return false;
        }
        self.creatures.borrow().contains_key(&target)
            && !self.corpses.borrow().contains(&target)
            && !self.pet_owners.borrow().contains_key(&target)
    }
    fn nearest_hostile_to(&self, pet: &Pet) -> Option<u64> {
        let creatures = self.creatures.borrow();
        creatures
            .iter()
            .filter(|(guid, c)| {
                self.pet_may_attack(pet, **guid)
                    && (c.map_id, c.instance_id) == (pet.map_id, pet.instance_id)
            })
            .min_by(|a, b| {
                dist_sq(a.1.at, pet.at)
                    .partial_cmp(&dist_sq(b.1.at, pet.at))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.0.cmp(b.0))
            })
            .map(|(guid, _)| *guid)
    }
    fn cancel_attack_order(&mut self, owner: u64) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::OrderCleared(owner));
        let mut bars = self.pet_bars.borrow_mut();
        if let Some((command, _)) = bars.get_mut(&owner) {
            *command = PetOrder::Follow;
        }
    }
    fn take_victim(&mut self, pet: u64, victim: u64) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::Took(pet, victim));
        let instance_id = self.creatures.borrow()[&pet].instance_id;
        let mut fights = self.fights.borrow_mut();
        match fights.iter_mut().find(|f| f.attacker == pet) {
            Some(f) => f.victim = victim,
            None => fights.push(Engagement {
                attacker: pet,
                victim,
                instance_id,
                player_never_swung: false,
            }),
        }
    }
    fn stand_down(&mut self, pet: u64) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::StoodDown(pet));
        self.fights.borrow_mut().retain(|f| f.attacker != pet);
    }
    fn dismiss(&mut self, pet: u64) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::Dismissed(pet));
        self.creatures.borrow_mut().remove(&pet);
        self.pet_owners.borrow_mut().remove(&pet);
        self.legs.borrow_mut().retain(|l| l.guid != pet);
        self.fights
            .borrow_mut()
            .retain(|f| f.attacker != pet && f.victim != pet);
    }
    fn restage(&mut self, pet: u64, at: Point, map_id: u32, instance_id: u64, now_ms: u32) {
        self.pet_effects
            .borrow_mut()
            .push(PetEffect::Restaged(pet, at, map_id, instance_id));
        self.place(pet, at, Some(now_ms), None);
        if let Some(c) = self.creatures.borrow_mut().get_mut(&pet) {
            c.map_id = map_id;
            c.instance_id = instance_id;
        }
    }
}

const WOLF: u64 = 0x0000_0000_0000_0BEE;
const PACK_MATE: u64 = WOLF + 1;
const FAR_MATE: u64 = WOLF + 2;
const HUNTER: u64 = 0x0000_0000_0000_0A11;
const RANGER: u64 = HUNTER + 1;
const IMP: u64 = 0x0000_0000_0000_1117;
const NUKE: u32 = 100;
const HEAL: u32 = 101;
const DEBUFF: u32 = 102;
const BUFF: u32 = 103;
const BEASTS: u32 = 14;
const ALLIANCE: u32 = 1;
const MAP: u32 = 0;
const INSTANCE: u64 = 0;
const LEG_MS: u32 = 1000;
const HALF_WAY: u64 = 500_000;
const MOVE_FLAG_TURN_LEFT: u32 = 0x10;

fn p(x: f32, y: f32, z: f32) -> Point {
    Point { x, y, z }
}

fn catch_all() -> TickScope {
    TickScope::from_rows(crate::creatures::GLOBAL_TICK_INSTANCE, [])
}

fn wolf_mid_flight(now_micros: u64) -> Scenario {
    Scenario::new(now_micros)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .flying(WOLF, p(0.0, 0.0, 10.0), p(10.0, 0.0, 10.0), 0, LEG_MS)
}

fn idle_wolf(at: Point, wanders: bool) -> Scenario {
    Scenario::new(HALF_WAY)
        .creature(WOLF, at)
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), wanders)
}

#[test]
fn a_creature_mid_flight_moves_to_the_point_its_client_renders() {
    let mut w = wolf_mid_flight(HALF_WAY);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let wolf = w.at(WOLF);
    assert_eq!(
        wolf.at,
        p(5.0, 0.0, 10.0),
        "the authoritative position tracks where the leg renders"
    );
    assert_eq!(
        (wolf.grid, wolf.cell),
        (
            spatial::grid_cell(5.0, 0.0),
            spatial::grid_cell_id(
                spatial::grid_cell(5.0, 0.0).0,
                spatial::grid_cell(5.0, 0.0).1
            )
        ),
        "grid address and packed cell move with the position"
    );
    assert_eq!(
        wolf.last_move_ms,
        (HALF_WAY / 1000) as u32,
        "a creature that travelled must stamp its move clock, or the idle passes treat it as parked"
    );
    assert!(
        w.has_leg(WOLF),
        "a leg still in flight must keep playing; forgetting it strands the creature mid-route"
    );
}

#[test]
fn a_dedicated_firing_advances_only_its_instance_splines() {
    let start = p(0.0, 0.0, 10.0);
    let dest = p(10.0, 0.0, 10.0);
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, start)
        .in_instance(WOLF, DEDICATED)
        .flying(WOLF, start, dest, 0, LEG_MS)
        .creature(PACK_MATE, start)
        .flying(PACK_MATE, start, dest, 0, LEG_MS);
    let tick = w.tick(false, TickScope::Only(DEDICATED));
    run_cycle(&mut w, tick);
    assert_eq!(w.at(WOLF).at, p(5.0, 0.0, 10.0));
    assert_eq!(w.at(PACK_MATE).at, start);

    let tick = w.tick(
        false,
        TickScope::from_rows(crate::creatures::GLOBAL_TICK_INSTANCE, [DEDICATED]),
    );
    run_cycle(&mut w, tick);
    assert_eq!(w.at(WOLF).at, p(5.0, 0.0, 10.0));
    assert_eq!(w.at(PACK_MATE).at, p(5.0, 0.0, 10.0));
}

#[test]
fn a_later_pass_reads_the_advanced_position_not_the_leg_start() {
    let mut w = wolf_mid_flight(HALF_WAY)
        .awake([WOLF])
        .route(WOLF, &[(1, p(-4.0, 0.0, 10.0)), (2, p(11.0, 0.0, 10.0))]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.seen_by_sweep.borrow().as_slice(),
        [(WOLF, p(0.0, 0.0, 10.0))],
        "the sweep runs before advance, so it still reads the leg start"
    );
    assert_eq!(
        w.effects().first().map(|e| e.dest),
        Some(p(11.0, 0.0, 10.0)),
        "later passes decide from the rendered position"
    );
}

#[test]
fn a_patrolling_creature_walks_the_next_segment_of_its_route() {
    let route = [(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))];
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .route(WOLF, &route)
        .walking_to(WOLF, 1);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let now_ms = (HALF_WAY / 1000) as u32;
    let grid = spatial::grid_cell(0.0, 0.0);
    assert_eq!(
        w.effects(),
        [MoveEffect {
            guid: WOLF,
            start: p(0.0, 0.0, 10.0),
            dest: p(10.0, 0.0, 10.0),
            dur_ms: 4000,
            spline_id: now_ms,
            run: false,
            map_id: MAP,
            instance_id: INSTANCE,
            grid,
            cell: spatial::grid_cell_id(grid.0, grid.1),
            facing: false,
            facing_angle: 0.0,
        }],
        "a patroller walks one route segment at walk pace"
    );
    assert_eq!(
        (w.at(WOLF).wp_target, w.at(WOLF).leg_ends_ms),
        (2, now_ms + 4000),
        "the cursor advances to the target waypoint and the leg is held to completion"
    );
}

#[test]
fn a_badly_displaced_patroller_steps_toward_its_waypoint_one_firing_at_a_time() {
    let route = [(1, p(5.0, 5.0, 10.0)), (2, p(0.0, 0.0, 10.0))];
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 300.0, 10.0))
        .awake([WOLF])
        .route(WOLF, &route)
        .walking_to(WOLF, 2);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let effect = w.effects()[0];
    assert_eq!(
        (effect.dest, effect.run, effect.dur_ms),
        (p(0.0, 298.75, 10.0), false, 500),
        "a displaced patroller walks one firing's worth toward its waypoint"
    );
    assert_eq!(
        (w.at(WOLF).wp_target, w.at(WOLF).leg_ends_ms),
        (2, 0),
        "the cursor stays on its waypoint and the step is not held to completion"
    );
}

#[test]
fn a_displaced_patroller_converges_then_resumes_ordinary_patrolling() {
    let route = [(1, p(5.0, 5.0, 10.0)), (2, p(0.0, 0.0, 10.0))];
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 41.0, 10.0))
        .awake([WOLF])
        .route(WOLF, &route)
        .walking_to(WOLF, 2);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    w.advance_clock(500_000);

    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()[0].dest,
        p(0.0, 39.75, 10.0),
        "the first firing must still be a bounded step, not a leap to the waypoint"
    );
    let now_ms = (HALF_WAY / 1000) as u32 + 500;
    assert_eq!(
        (
            w.effects()[1].dest,
            w.effects()[1].dur_ms,
            w.at(WOLF).leg_ends_ms
        ),
        (p(0.0, 0.0, 10.0), 15900, now_ms + 15900),
        "once close, patrolling resumes one held leg to the waypoint"
    );
}

#[test]
fn a_patroller_outside_every_active_cell_stays_frozen_on_its_route() {
    let route = [(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))];
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .route(WOLF, &route)
        .walking_to(WOLF, 1);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty() && w.at(WOLF).wp_target == 1,
        "an unseen creature costs nothing and keeps its route state"
    );
}

#[test]
fn a_displaced_creature_runs_home_one_firing_at_a_time() {
    let mut w = idle_wolf(p(0.0, 20.0, 10.0), false);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let effect = w.effects()[0];
    assert_eq!(
        (effect.dest, effect.run, effect.dur_ms),
        (p(0.0, 16.5, 10.0), true, 500),
        "the walk home is one firing's run toward the post"
    );
}

#[test]
fn a_creature_that_runs_home_does_not_also_wander_in_the_same_firing() {
    let mut w = idle_wolf(p(0.0, 20.0, 10.0), true).rolls([0, 0, 0]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects().len(),
        1,
        "one leg per firing: two would share a spline id"
    );
    assert!(w.effects()[0].run, "walking home wins over loitering");
}

#[test]
fn a_creature_that_reaches_home_loiters_again() {
    let mut w = idle_wolf(p(0.0, 16.0, 10.0), true).rolls([0, 0, 0]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    w.advance_clock(500_000);

    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.at(WOLF).at,
        p(0.0, 12.5, 10.0),
        "the second cycle must advance the creature onto the leg it was given, not re-issue it"
    );
    assert!(
        !w.effects()[1].run,
        "a creature back home loiters at walk pace"
    );
}

#[test]
fn an_idle_creature_loiters_near_its_post_on_about_a_third_of_firings() {
    for (chance_roll, hops) in [(0u32, true), (99, false)] {
        let mut w = idle_wolf(p(0.0, 0.0, 10.0), true).rolls([chance_roll, 0, u32::MAX]);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            usize::from(hops),
            "a loiterer pauses between hops (roll {chance_roll})"
        );
    }

    let mut w = idle_wolf(p(0.0, 0.0, 10.0), true).rolls([0, 0, u32::MAX]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    let effect = w.effects()[0];
    assert_eq!(
        (effect.dest, effect.run, effect.dur_ms),
        (p(WANDER_RADIUS, 0.0, 10.0), false, 2400),
        "the hop stays inside the post's wander radius at walk pace"
    );
    assert_eq!(
        w.at(WOLF).leg_ends_ms,
        (HALF_WAY / 1000) as u32 + 2400,
        "the hop is held to completion"
    );
}

#[test]
fn loitering_waits_for_a_sense_firing_but_walking_home_does_not() {
    let mut loiterer = idle_wolf(p(0.0, 0.0, 10.0), true).rolls([0, 0, u32::MAX]);
    let tick = loiterer.tick(false, catch_all());
    run_cycle(&mut loiterer, tick);

    let mut displaced = idle_wolf(p(0.0, 20.0, 10.0), false);
    let tick = displaced.tick(false, catch_all());
    run_cycle(&mut displaced, tick);

    assert_eq!(
        (loiterer.effects().len(), displaced.effects().len()),
        (0, 1),
        "the hop chance rolls per sense firing; the walk home steps every firing"
    );
}

#[test]
fn an_idle_creature_that_holds_its_post_never_loiters() {
    let mut w = idle_wolf(p(0.0, 0.0, 10.0), false).rolls([0, 0, 0]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty(),
        "an IDLE-movement creature stays on its post"
    );
}

#[test]
fn a_creature_with_a_leg_still_in_flight_starts_no_new_leg() {
    let now_ms = (HALF_WAY / 1000) as u32;
    let route = [(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))];
    let mut patroller = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .route(WOLF, &route)
        .walking_to(WOLF, 1)
        .mid_leg(WOLF, now_ms + 1);
    let tick = patroller.tick(true, catch_all());
    run_cycle(&mut patroller, tick);

    let mut loiterer = idle_wolf(p(0.0, 0.0, 10.0), true)
        .mid_leg(WOLF, now_ms + 1)
        .rolls([0, 0, 0]);
    let tick = loiterer.tick(true, catch_all());
    run_cycle(&mut loiterer, tick);

    assert!(
        patroller.effects().is_empty() && loiterer.effects().is_empty(),
        "a leg still animating is not re-thrown"
    );
}

#[test]
fn an_engaged_or_suppressed_creature_is_moved_by_no_idle_behavior() {
    let route = [(1, p(0.0, 0.0, 10.0)), (2, p(10.0, 0.0, 10.0))];
    let holds: [fn(Scenario, u64) -> Scenario; 2] = [Scenario::fighting, Scenario::rooted];
    for held in holds {
        for patrols in [true, false] {
            let mut w = Scenario::new(HALF_WAY)
                .creature(WOLF, p(0.0, 20.0, 10.0))
                .awake([WOLF])
                .home(WOLF, p(0.0, 0.0, 10.0), true);
            if patrols {
                w = w.route(WOLF, &route).walking_to(WOLF, 1);
            }
            let mut w = held(w, WOLF);
            let tick = w.tick(true, catch_all());
            run_cycle(&mut w, tick);

            assert!(
                w.effects().is_empty(),
                "an idle mover throws no leg while chasing or feared (patrols={patrols})"
            );
            if patrols {
                assert_eq!(
                    w.at(WOLF).wp_target,
                    1,
                    "a held creature keeps its route state"
                );
            }
        }
    }
}

#[test]
fn a_leg_lands_on_the_ground_under_its_destination() {
    let mut snapped = idle_wolf(p(0.0, 20.0, 10.0), false).ground(42.0);
    let tick = snapped.tick(true, catch_all());
    run_cycle(&mut snapped, tick);

    assert_eq!(
        (snapped.effects()[0].dest.z, {
            let mut off_slice = idle_wolf(p(0.0, 20.0, 10.0), false);
            let tick = off_slice.tick(true, catch_all());
            run_cycle(&mut off_slice, tick);
            off_slice.effects()[0].dest.z
        }),
        (42.0, 10.0),
        "a leg lands on imported ground, else at the post's height"
    );
}

#[test]
fn a_blocked_walk_home_goes_around_instead_of_through() {
    let mut w = idle_wolf(p(0.0, 20.0, 10.0), false).detour(WOLF, (20.0, 20.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()[0].dest,
        p(3.5, 20.0, 10.0),
        "the walk home follows the navigation detour"
    );
}

#[test]
fn an_arrived_creature_stops_on_its_destination_and_the_leg_is_forgotten() {
    let mut w = wolf_mid_flight(LEG_MS as u64 * 1000);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.at(WOLF).at,
        p(10.0, 0.0, 10.0),
        "an arrived creature must land exactly on its destination"
    );
    assert!(
        !w.has_leg(WOLF),
        "a landed leg that is not forgotten replays forever and the creature never goes idle"
    );
}

#[test]
fn retained_waypoints_cross_a_short_segment_and_turn_in_one_firing() {
    let mut w = Scenario::new(1_000_000)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .flying(WOLF, p(0.0, 0.0, 10.0), p(0.5, 7.0, 10.0), 0, 7_500);
    w.legs.borrow_mut()[0].waypoints = vec![(0.5, 0.0, 10.0), (0.5, 7.0, 10.0)];
    let tick = w.tick(false, catch_all());
    advance_legs(&mut w, &tick);
    assert_eq!(w.at(WOLF).at, p(0.5, 0.5, 10.0));
    assert!(w.has_leg(WOLF));
    assert!(
        w.effects().is_empty(),
        "crossing a waypoint must not replace the client path"
    );
    w.advance_clock(6_500_000);
    let tick = w.tick(false, catch_all());
    advance_legs(&mut w, &tick);
    assert_eq!(w.at(WOLF).at, p(0.5, 7.0, 10.0));
    assert!(!w.has_leg(WOLF));
}

#[test]
fn a_new_obstruction_stops_a_retained_path_before_committing_the_next_position() {
    let mut w = Scenario::new(1_000_000)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .flying(WOLF, p(0.0, 0.0, 10.0), p(0.5, 7.0, 10.0), 0, 7_500);
    w.legs.borrow_mut()[0].waypoints = vec![(0.5, 0.0, 10.0), (0.5, 7.0, 10.0)];
    w.obstructed_paths.borrow_mut().insert(WOLF);
    let tick = w.tick(false, catch_all());
    advance_legs(&mut w, &tick);
    assert_eq!(w.at(WOLF).at, p(0.0, 0.0, 10.0));
    assert_eq!(w.effects()[0].dest, p(0.0, 0.0, 10.0));
    assert_eq!(w.effects()[0].dur_ms, 0);
}

fn wolf_on_a_route_path(stored: Point) -> Scenario {
    let w = Scenario::new(1_500_000).creature(WOLF, stored).flying(
        WOLF,
        p(0.0, 0.0, 10.0),
        p(10.0, 10.0, 10.0),
        0,
        4_000,
    );
    w.legs.borrow_mut()[0].waypoints = vec![(10.0, 0.0, 10.0), (10.0, 10.0, 10.0)];
    w
}

fn assert_near(at: Point, want: Point) {
    assert!(
        (at.x - want.x).abs() < 1e-4 && (at.y - want.y).abs() < 1e-4 && at.z == want.z,
        "expected {want:?}, stopped at {at:?}"
    );
}

#[test]
fn a_stop_after_a_segment_end_holds_the_rendered_point_facing_the_next_segment() {
    let w = wolf_on_a_route_path(p(7.5, 0.0, 10.0));
    let leg = w.legs.borrow()[0].clone();
    let stop = stop_on_leg(&w, &leg, 2_300_000).expect("a finite stop");
    assert_near(stop.at, p(10.0, 1.5, 10.0));
    assert_eq!(
        stop.heading,
        Some(std::f32::consts::FRAC_PI_2),
        "facing along +y"
    );
}

#[test]
fn a_stop_after_the_route_path_ends_holds_its_destination() {
    let w = wolf_on_a_route_path(p(10.0, 7.5, 10.0));
    let leg = w.legs.borrow()[0].clone();
    let stop = stop_on_leg(&w, &leg, 4_200_000).expect("a finite stop");
    assert_near(stop.at, p(10.0, 10.0, 10.0));
    assert_eq!(stop.heading, None, "a landed path names no next waypoint");
}

#[test]
fn a_stop_on_an_obstructed_route_path_keeps_the_stored_point() {
    let w = wolf_on_a_route_path(p(7.5, 0.0, 10.0));
    w.obstructed_paths.borrow_mut().insert(WOLF);
    let leg = w.legs.borrow()[0].clone();
    assert_eq!(
        stop_on_leg(&w, &leg, 2_300_000),
        Some(Stop {
            at: p(7.5, 0.0, 10.0),
            heading: None,
        }),
        "a stop must halt where the advance would, never past the obstruction"
    );
}

#[test]
fn a_stop_on_a_non_finite_leg_keeps_the_stored_point() {
    let w = Scenario::new(1_500_000)
        .creature(WOLF, p(7.5, 0.0, 10.0))
        .flying(WOLF, p(0.0, 0.0, 10.0), p(f32::NAN, 0.0, 10.0), 0, 4_000);
    let leg = w.legs.borrow()[0].clone();
    assert_eq!(stop_on_leg(&w, &leg, 2_300_000), None);
}

#[test]
fn crowd_control_interrupts_a_retained_path_at_its_current_turn() {
    let mut w = Scenario::new(1_000_000)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .flying(WOLF, p(0.0, 0.0, 10.0), p(0.5, 7.0, 10.0), 0, 7_500)
        .rooted(WOLF);
    w.legs.borrow_mut()[0].waypoints = vec![(0.5, 0.0, 10.0), (0.5, 7.0, 10.0)];
    let tick = w.tick(false, catch_all());
    advance_legs(&mut w, &tick);
    assert_eq!(w.at(WOLF).at, p(0.5, 0.5, 10.0));
    assert_eq!(w.effects().len(), 1);
    assert_eq!(w.effects()[0].dur_ms, 0);
    assert_eq!(w.effects()[0].dest, p(0.5, 0.5, 10.0));
}

#[test]
fn a_movement_suppressed_creature_freezes_where_it_renders() {
    let mut w = wolf_mid_flight(HALF_WAY).rooted(WOLF);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let wolf = w.at(WOLF);
    assert_eq!(
        wolf.at,
        p(5.0, 0.0, 10.0),
        "a rooted creature must stop where it renders, not slide on to the leg destination"
    );
    assert_eq!(
        wolf.last_move_ms, 0,
        "a frozen creature did not travel, so its move clock must not advance"
    );

    let grid = spatial::grid_cell(5.0, 0.0);
    assert_eq!(
        w.effects(),
        [MoveEffect {
            guid: WOLF,
            start: p(5.0, 0.0, 10.0),
            dest: p(5.0, 0.0, 10.0),
            dur_ms: 0,
            spline_id: (HALF_WAY / 1000) as u32,
            run: false,
            map_id: MAP,
            instance_id: INSTANCE,
            grid,
            cell: spatial::grid_cell_id(grid.0, grid.1),
            facing: false,
            facing_angle: 0.0,
        }],
        "a rooted creature's client stops where the server froze it"
    );
}

#[test]
fn a_non_finite_leg_is_refused_and_the_creature_stays_put() {
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .flying(WOLF, p(0.0, 0.0, 10.0), p(f32::NAN, 0.0, 10.0), 0, LEG_MS);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.at(WOLF).at,
        p(0.0, 0.0, 10.0),
        "a leg to a non-finite position is refused"
    );
    assert!(
        !w.has_leg(WOLF),
        "the corrupt leg must not survive the cycle"
    );
    assert!(
        w.effects().is_empty(),
        "a refused leg must relay nothing to the client"
    );
}

#[test]
fn a_leg_whose_mover_despawned_is_reaped() {
    let mut w =
        Scenario::new(HALF_WAY).flying(WOLF, p(0.0, 0.0, 10.0), p(10.0, 0.0, 10.0), 0, LEG_MS);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        !w.has_leg(WOLF),
        "a leg belonging to a despawned creature must be reaped, or it accumulates forever"
    );
    assert!(
        w.effects().is_empty(),
        "a despawned creature must not be relayed as moving"
    );
}

#[test]
fn world_maintenance_and_package_passes_run_on_every_catch_all_firing() {
    for (sense, scope, maintenance, packages) in [
        (true, catch_all(), 1, 1),
        (false, catch_all(), 1, 1),
        (true, TickScope::Only(7), 0, 0),
    ] {
        let mut w = wolf_mid_flight(HALF_WAY);
        let tick = w.tick(sense, scope);
        run_cycle(&mut w, tick);
        assert_eq!(
            (w.maintenance_runs.get(), w.package_runs.get()),
            (maintenance, packages),
            "a dedicated instance row runs no world passes (sense={sense})"
        );
    }
}

fn wolf_and_player(player_at: Point) -> Scenario {
    Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, player_at)
}

type Twist = fn(Scenario) -> Scenario;

type CastEffect = (u64, u32, u64);

fn pack_mate(w: Scenario, guid: u64, at: Point) -> Scenario {
    w.creature(guid, at).tuned_aggro_range(guid, 1)
}

#[test]
fn a_creature_engages_the_nearest_hostile_player_it_can_see() {
    let mut w = wolf_and_player(p(15.0, 0.0, 10.0)).player(RANGER, p(3.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pulls(),
        [(WOLF, RANGER, Pull::Noticed)],
        "a creature pulls only the closest player it notices"
    );
}

#[test]
fn a_godmoded_or_dead_player_draws_no_aggro() {
    let untouchable: [fn(Scenario, u64) -> Scenario; 2] = [Scenario::godmoded, Scenario::corpse];
    for hold in untouchable {
        let mut w = hold(wolf_and_player(p(5.0, 0.0, 10.0)), HUNTER);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert!(
            w.pulls().is_empty(),
            "a player who cannot die is never pulled"
        );
    }
}

#[test]
fn a_creature_outside_every_active_cell_notices_nobody() {
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(2.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.pulls().is_empty(),
        "only creatures in active cells scan for aggro"
    );
}

#[test]
fn a_stealthed_player_is_noticed_only_from_inside_the_detection_range() {
    for (dist, noticed) in [(4.0f32, true), (9.0, false)] {
        let mut w = wolf_and_player(p(dist, 0.0, 10.0)).stealthed(HUNTER);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            !w.pulls().is_empty(),
            noticed,
            "stealth shrinks the aggro radius with distance ({dist} yards)"
        );
    }
}

#[test]
fn the_aggro_radius_scales_with_level_honors_the_template_override_and_greys_out() {
    let cases = [
        (
            "equal levels, inside the base radius",
            10,
            10,
            None,
            19.0,
            true,
        ),
        ("equal levels, outside it", 10, 10, None, 21.0, false),
        (
            "the creature out-levels the player",
            15,
            10,
            None,
            24.0,
            true,
        ),
        (
            "a hand-tuned template range wins",
            10,
            10,
            Some(8),
            12.0,
            false,
        ),
        (
            "grey: the player out-levels it too far",
            5,
            20,
            None,
            3.0,
            false,
        ),
    ];
    for (case, creature_level, player_level, tuned, dist, noticed) in cases {
        let mut w = wolf_and_player(p(dist, 0.0, 10.0))
            .level(WOLF, creature_level)
            .player_level(HUNTER, player_level);
        if let Some(yards) = tuned {
            w = w.tuned_aggro_range(WOLF, yards);
        }
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            !w.pulls().is_empty(),
            noticed,
            "aggro range follows level difference and template overrides ({case})"
        );
    }
}

#[test]
fn a_wall_between_them_stops_the_pull() {
    let mut w = wolf_and_player(p(5.0, 0.0, 10.0)).wall_between(WOLF, HUNTER);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(w.pulls().is_empty(), "no pull without line of sight");
}

#[test]
fn a_soothed_crowd_controlled_or_near_death_creature_starts_no_fight() {
    let quiet: [(&str, Twist); 3] = [
        ("soothed", |w| w.soothed(WOLF, -20.0)),
        ("crowd controlled", |w| w.crowd_controlled(WOLF)),
        ("near death", |w| w.near_death(WOLF)),
    ];
    for (case, hold) in quiet {
        let mut w = hold(wolf_and_player(p(5.0, 0.0, 10.0)));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert!(
            w.pulls().is_empty(),
            "Mind Soothe, stuns and a pending rout stop the pull ({case})"
        );
    }
}

#[test]
fn a_pack_mate_answers_a_call_it_can_hear() {
    let mut w = pack_mate(
        wolf_and_player(p(5.0, 0.0, 10.0)),
        PACK_MATE,
        p(0.0, 8.0, 10.0),
    );
    w = pack_mate(w, FAR_MATE, p(0.0, 40.0, 10.0)).awake([WOLF, PACK_MATE, FAR_MATE]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pulls(),
        [
            (WOLF, HUNTER, Pull::Noticed),
            (PACK_MATE, HUNTER, Pull::Assisted)
        ],
        "assist reaches only neighbors within range of the caller"
    );
}

#[test]
fn a_neighbor_of_the_wrong_kind_place_or_sight_ignores_the_call() {
    let cases: [(&str, Twist); 3] = [
        ("another faction", |w| w.faction(PACK_MATE, 99)),
        ("another instance", |w| w.in_instance(PACK_MATE, 7)),
        ("a wall between them", |w| w.wall_between(PACK_MATE, WOLF)),
    ];
    for (case, apart) in cases {
        let mut w = apart(
            pack_mate(
                wolf_and_player(p(5.0, 0.0, 10.0)),
                PACK_MATE,
                p(0.0, 8.0, 10.0),
            )
            .awake([WOLF, PACK_MATE]),
        );
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.pulls(),
            [(WOLF, HUNTER, Pull::Noticed)],
            "assist needs same kind, same partition and line of sight ({case})"
        );
    }
}

#[test]
fn nobody_pulling_means_no_assist_scan_at_all() {
    let mut w = pack_mate(
        Scenario::new(HALF_WAY)
            .creature(WOLF, p(0.0, 0.0, 10.0))
            .at_war(BEASTS, ALLIANCE)
            .player(HUNTER, p(100.0, 0.0, 10.0)),
        PACK_MATE,
        p(0.0, 5.0, 10.0),
    )
    .awake([WOLF, PACK_MATE]);
    let tick = w.tick(true, catch_all());
    let outcome = run_cycle(&mut w, tick);

    assert_eq!(
        outcome
            .rows_visited
            .iter()
            .find(|(key, _)| *key == "assist"),
        Some(&("assist", 0)),
        "a firing with no pull must not walk the active set"
    );
}

#[test]
fn a_neighbor_between_two_callers_answers_the_lower_guid() {
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(-3.0, 0.0, 10.0))
        .creature(PACK_MATE, p(3.0, 0.0, 10.0))
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(-3.0, 2.0, 10.0))
        .player(RANGER, p(3.0, 2.0, 10.0));
    w = pack_mate(w, FAR_MATE, p(0.0, 0.0, 10.0)).awake([WOLF, PACK_MATE, FAR_MATE]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pulls(),
        [
            (WOLF, HUNTER, Pull::Noticed),
            (PACK_MATE, RANGER, Pull::Noticed),
            (FAR_MATE, HUNTER, Pull::Assisted)
        ],
        "a neighbor between two callers joins the same one every run"
    );
}

#[test]
fn every_unit_in_a_covered_fight_is_flagged_in_combat_and_an_aiming_player_is_not() {
    for (scope, flagged) in [
        (catch_all(), vec![HUNTER, WOLF]),
        (TickScope::Only(7), Vec::new()),
    ] {
        let mut w = Scenario::new(HALF_WAY)
            .creature(WOLF, p(0.0, 0.0, 10.0))
            .player(HUNTER, p(2.0, 0.0, 10.0))
            .player(RANGER, p(30.0, 0.0, 10.0))
            .attacking(WOLF, HUNTER)
            .aiming_at(RANGER, WOLF);
        let tick = w.tick(false, scope);
        run_cycle(&mut w, tick);

        assert_eq!(
            w.flagged(),
            flagged,
            "both sides of a fight stay flagged every firing; an auto-attack toggle at range is not combat"
        );
    }
}

#[test]
fn a_creature_that_pulls_is_in_combat_before_the_same_cycle_ends() {
    let mut w = wolf_and_player(p(5.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.flagged(),
        [HUNTER, WOLF],
        "a pulling creature is flagged in combat in the same firing"
    );
}

const SETTLED: u64 = 4_000_000;

// Vanilla chase stop: two 0.389 yd bounding radii plus the 0.5 yd contact distance.
const CONTACT_YD: f32 = 1.278;

fn wolf_fighting(player_at: Point) -> Scenario {
    Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, player_at)
}

fn near(at: Point, want: Point, tolerance: f32) -> bool {
    (at.x - want.x).abs() < tolerance && (at.y - want.y).abs() < tolerance && at.z == want.z
}

fn assert_running_legs_to(w: &Scenario, want: &[Point], why: &str) {
    let legs = w.effects();
    assert_eq!(legs.len(), want.len(), "{why}: {legs:?}");
    for (leg, want) in legs.iter().zip(want) {
        assert!(
            leg.run && !leg.facing && leg.dur_ms > 0 && near(leg.dest, *want, 1e-4),
            "{why}: expected a running leg to {want:?}, got {leg:?}"
        );
    }
}

#[test]
fn a_creature_that_notices_a_player_closes_on_it_in_the_same_cycle() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0)).awake([WOLF]);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(w.pulls(), [(WOLF, HUNTER, Pull::Noticed)]);
    assert_running_legs_to(
        &w,
        &[p(15.0 - CONTACT_YD, 0.0, 10.0)],
        "a pulling creature runs to its contact point in the same firing",
    );
}

#[test]
fn an_engaged_creature_outside_every_active_cell_still_chases() {
    for (scope, legs) in [(catch_all(), 1), (TickScope::Only(7), 0)] {
        let mut w = wolf_fighting(p(15.0, 0.0, 10.0)).attacking(WOLF, HUNTER);
        let tick = w.tick(false, scope);
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            legs,
            "the engagement drives the fight, once per covering firing"
        );
    }
}

#[test]
fn a_victim_past_the_pursuit_cutoff_is_left_alone() {
    let cutoff = CHASE_LEASH_SQ.sqrt();
    for (dist, legs) in [(cutoff - 5.0, 1), (cutoff + 5.0, 0)] {
        let mut w = wolf_fighting(p(dist, 0.0, 10.0)).attacking(WOLF, HUNTER);
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            legs,
            "a chaser stops at the pursuit cutoff ({dist} yards)"
        );
    }
}

#[test]
fn a_chaser_rides_one_committed_leg_until_its_victim_veers() {
    let mut w = wolf_fighting(p(20.0, 0.0, 10.0))
        .kiting(HUNTER, p(20.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    w.advance_clock(500_000);
    w = w.kiting(HUNTER, p(23.0, 0.0, 10.0));
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        1,
        "a chaser rides its committed leg while the kiter holds heading"
    );

    w.advance_clock(500_000);
    w = w.kiting(HUNTER, p(23.0, 20.0, 10.0));
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    let legs = w.effects();
    assert_eq!(legs.len(), 2, "a chaser re-aims once the victim veers");
    assert!(
        legs[1].spline_id > legs[0].spline_id,
        "the replacement leg carries a newer spline id"
    );
}

#[test]
fn a_creature_that_reaches_a_standing_victim_stops_and_faces_it() {
    let launched = SETTLED - 500_000;
    let mut w = wolf_fighting(p(7.0, 0.0, 10.0))
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER)
        .flying(
            WOLF,
            p(0.0, 0.0, 10.0),
            p(10.0, 0.0, 10.0),
            launched,
            LEG_MS,
        );
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let rendered = p(5.0, 0.0, 10.0);
    let stop = w.effects();
    assert_eq!(
        stop.iter()
            .map(|e| (e.start, e.dest, e.dur_ms, e.facing, e.facing_angle))
            .collect::<Vec<_>>(),
        [(rendered, rendered, 0, true, 0.0)],
        "stop and turn are one facing spline at the rendered point"
    );
    assert!(
        stop[0].spline_id > (launched / 1000) as u32,
        "the stop carries a newer spline id"
    );
    assert_eq!(
        (w.at(WOLF).at, w.at(WOLF).orientation),
        (rendered, 0.0),
        "server and client must agree on where the creature stopped and which way it now looks"
    );

    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(w.effects().len(), 1, "a settled fight sends nothing");
}

#[test]
fn a_creature_whose_victim_kites_out_of_reach_keeps_chasing() {
    let mut w = wolf_fighting(p(4.0, 0.0, 10.0))
        .kiting(HUNTER, p(4.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_running_legs_to(
        &w,
        &[p(4.0 - CONTACT_YD, 0.0, 10.0)],
        "a chaser keeps running to the contact point instead of planting",
    );
}

const STALE_MOVE_WINDOW_MS: u32 = 700;

const APPROACHES: [f32; 8] = [
    0.0,
    std::f32::consts::FRAC_PI_4,
    std::f32::consts::FRAC_PI_2,
    2.356_194_5,
    std::f32::consts::PI,
    -2.356_194_5,
    -std::f32::consts::FRAC_PI_2,
    -std::f32::consts::FRAC_PI_4,
];

#[test]
fn a_standing_victim_inside_reach_is_turned_to_from_every_approach() {
    for bearing in APPROACHES {
        let at = p(3.0 * bearing.cos(), 3.0 * bearing.sin(), 10.0);
        let mut w = wolf_fighting(at).facing(WOLF, 0.0).attacking(WOLF, HUNTER);
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        let effects = w.effects();
        assert!(
            effects.iter().all(|e| e.facing),
            "a creature already in reach does not move ({bearing} rad)"
        );
        assert!(
            effects.len() <= 1 && w.at(WOLF).at == p(0.0, 0.0, 10.0),
            "a creature in reach turns at most once and stays put ({bearing} rad)"
        );
    }
}

#[test]
fn a_player_who_just_released_the_movement_key_is_standing_at_once() {
    let mut w = wolf_fighting(p(3.0, 0.0, 10.0))
        .kiting(HUNTER, p(3.0, 0.0, 10.0))
        .stopped(HUNTER)
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER);
    assert!(
        w.since_last_move_ms(HUNTER) < STALE_MOVE_WINDOW_MS,
        "the point of this scenario is a move clock that still reads FRESH"
    );
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()
            .iter()
            .map(|e| (e.facing, e.dur_ms))
            .collect::<Vec<_>>(),
        [(true, 0)],
        "a creature turns to a victim that stopped in reach"
    );
}

#[test]
fn a_player_turning_in_place_is_not_translating() {
    let mut w = wolf_fighting(p(3.0, 0.0, 10.0))
        .turning(HUNTER)
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()
            .iter()
            .map(|e| (e.facing, e.dur_ms))
            .collect::<Vec<_>>(),
        [(true, 0)],
        "a creature squares up to a player who only turns"
    );
}

#[test]
fn a_kiter_inside_reach_is_chased_without_a_stop_between_firings() {
    let mut w = wolf_fighting(p(4.0, 0.0, 10.0))
        .kiting(HUNTER, p(4.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER);
    for step in 0..3 {
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);
        w.advance_clock(500_000);
        w = w.kiting(HUNTER, p(7.0 + 3.0 * step as f32, 0.0, 10.0));
    }

    assert_running_legs_to(
        &w,
        &[
            p(4.0 - CONTACT_YD, 0.0, 10.0),
            p(7.0 - CONTACT_YD, 0.0, 10.0),
            p(10.0 - CONTACT_YD, 0.0, 10.0),
        ],
        "a moving victim is run down without stops, each leg ending at contact distance",
    );
}

#[test]
fn a_victim_moving_inside_reach_is_never_run_into_or_past_from_any_approach() {
    for bearing in APPROACHES {
        let victim = p(3.0 * bearing.cos(), 3.0 * bearing.sin(), 10.0);
        let mut w = wolf_fighting(victim)
            .kiting(HUNTER, victim)
            .attacking(WOLF, HUNTER);
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        let stop = p(
            (3.0 - CONTACT_YD) * bearing.cos(),
            (3.0 - CONTACT_YD) * bearing.sin(),
            10.0,
        );
        assert_running_legs_to(
            &w,
            &[stop],
            &format!(
                "a leg at a player moving in melee ends on the creature's side ({bearing} rad)"
            ),
        );
    }
}

#[test]
fn a_victim_moving_inside_contact_distance_is_turned_to_not_chased() {
    let mut w = wolf_fighting(p(1.0, 0.0, 10.0))
        .kiting(HUNTER, p(1.0, 0.0, 10.0))
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects()
            .iter()
            .map(|e| (e.dest, e.dur_ms, e.facing, e.facing_angle))
            .collect::<Vec<_>>(),
        [(p(0.0, 0.0, 10.0), 0, true, 0.0)],
        "a creature in contact with a moving player holds and turns"
    );
}

// 0.3 yd is inside two 0.389 yd bounding radii; at SETTLED the wolf has stood still for 4 s.
fn wolf_with_a_character_inside_it() -> Scenario {
    wolf_fighting(p(0.3, 0.0, 10.0)).attacking(WOLF, HUNTER)
}

#[test]
fn a_creature_walks_back_out_of_a_character_standing_inside_it() {
    let mut w = wolf_with_a_character_inside_it();
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let legs = w.effects();
    assert!(
        legs.len() == 1
            && !legs[0].run
            && !legs[0].facing
            && legs[0].dur_ms > 0
            && near(legs[0].dest, p(0.3 - CONTACT_YD, 0.0, 10.0), 1e-4),
        "a creature inside a Character walks straight back to contact distance: {legs:?}"
    );
}

#[test]
fn a_creature_that_moved_within_two_seconds_holds_its_ground() {
    let mut w = wolf_with_a_character_inside_it().tweak(WOLF, |c| {
        c.last_move_ms = (SETTLED / 1000) as u32 - 1_999;
    });
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().iter().all(|e| e.facing),
        "a creature steps back only after standing still for 2 s"
    );
}

#[test]
fn a_creature_that_walked_back_out_of_a_character_turns_to_face_them_again() {
    let mut w = wolf_with_a_character_inside_it();
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    let effects = w.effects();
    assert!(
        effects.len() == 2
            && effects[1].facing
            && effects[1].facing_angle == 0.0
            && near(effects[1].dest, p(0.3 - CONTACT_YD, 0.0, 10.0), 1e-4),
        "after the walk back the next firing turns the creature to the Character: {effects:?}"
    );
}

#[test]
fn a_creature_that_only_turned_still_walks_back_out_of_a_character() {
    let mut w = wolf_with_a_character_inside_it()
        .facing(WOLF, std::f32::consts::PI)
        .tweak(WOLF, |c| c.last_move_ms = (SETTLED / 1000) as u32 - 1_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    w.advance_clock(1_500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    let effects = w.effects();
    assert!(
        effects.len() == 2 && effects[0].facing && !effects[1].facing && effects[1].dur_ms > 0,
        "turning in place is standing still; a turn must not restart the 2 s wait: {effects:?}"
    );
}

#[test]
fn a_creature_pulled_mid_patrol_does_not_walk_on_through_the_character() {
    let mut w = wolf_with_a_character_inside_it()
        .flying(WOLF, p(0.0, 0.0, 10.0), p(10.0, 0.0, 10.0), SETTLED, 4_000)
        .mid_leg(WOLF, (SETTLED / 1000) as u32 + 4_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let effects = w.effects();
    assert!(
        effects.len() == 1 && effects[0].facing && effects[0].dur_ms == 0,
        "the held patrol leg it was walking when pulled must stop at the Character: {effects:?}"
    );
}

#[test]
fn a_casting_creature_does_not_walk_back_out_of_a_character() {
    let mut w = wolf_with_a_character_inside_it().mid_cast(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().iter().all(|e| e.start == e.dest),
        "a cast roots its caster; cmangos does not step a casting chaser"
    );
}

#[test]
fn a_creature_does_not_walk_back_out_of_another_creature() {
    let mut w = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .creature(PACK_MATE, p(0.3, 0.0, 10.0))
        .attacking(WOLF, PACK_MATE);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().iter().all(|e| e.facing),
        "creatures back off only from Characters"
    );
}

#[test]
fn a_walk_back_rides_out_a_firing_that_comes_early() {
    let mut w = wolf_with_a_character_inside_it();
    let tick = w.firing(SETTLED, crate::creatures::MOVE_TICK_MICROS, catch_all());
    run_cycle(&mut w, tick);

    let tick = w.firing(
        SETTLED + 100_000,
        crate::creatures::MOVE_TICK_MICROS,
        catch_all(),
    );
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        1,
        "a lagged firing still finishes the walk back"
    );

    let tick = w.firing(
        SETTLED + 500_000,
        crate::creatures::MOVE_TICK_MICROS,
        catch_all(),
    );
    run_cycle(&mut w, tick);
    assert!(
        near(w.at(WOLF).at, p(0.3 - CONTACT_YD, 0.0, 10.0), 1e-4),
        "the walk back must reach the contact point: {:?}",
        w.at(WOLF).at
    );
}

#[test]
fn a_creature_that_starts_casting_stops_walking_back() {
    let mut w = wolf_with_a_character_inside_it();
    let tick = w.firing(SETTLED, crate::creatures::MOVE_TICK_MICROS, catch_all());
    run_cycle(&mut w, tick);

    w = w.mid_cast(WOLF);
    let tick = w.firing(
        SETTLED + 100_000,
        crate::creatures::MOVE_TICK_MICROS,
        catch_all(),
    );
    run_cycle(&mut w, tick);
    let effects = w.effects();
    assert!(
        effects.len() == 2 && effects[1].facing && effects[1].dur_ms == 0,
        "a cast that begins mid-walk roots the caster where it stands: {effects:?}"
    );
}

#[test]
fn a_creature_that_starts_casting_stops_walking_back_while_its_victim_moves_off() {
    let mut w = wolf_with_a_character_inside_it().caster(WOLF, 30.0);
    let tick = w.firing(SETTLED, crate::creatures::MOVE_TICK_MICROS, catch_all());
    run_cycle(&mut w, tick);

    w = w.mid_cast(WOLF).kiting(HUNTER, p(1.0, 0.0, 10.0));
    let tick = w.firing(
        SETTLED + 200_000,
        crate::creatures::MOVE_TICK_MICROS,
        catch_all(),
    );
    run_cycle(&mut w, tick);
    let effects = w.effects();
    assert!(
        effects.len() == 2 && effects[1].facing && effects[1].dur_ms == 0,
        "a cast roots the caster wherever its victim has gone: {effects:?}"
    );
}

#[test]
fn a_creature_that_switches_victim_stops_walking_back() {
    let mut w = wolf_with_a_character_inside_it().player(RANGER, p(0.3, 2.0, 10.0));
    let tick = w.firing(SETTLED, crate::creatures::MOVE_TICK_MICROS, catch_all());
    run_cycle(&mut w, tick);

    w.retarget(WOLF, RANGER);
    let tick = w.firing(
        SETTLED + 100_000,
        crate::creatures::MOVE_TICK_MICROS,
        catch_all(),
    );
    run_cycle(&mut w, tick);
    let effects = w.effects();
    assert!(
        effects.len() == 2 && effects[1].facing && effects[1].dur_ms == 0,
        "a new victim gets a fresh decision: {effects:?}"
    );
}

#[test]
fn a_chase_leg_is_stopped_where_it_renders_once_the_victim_stops() {
    let mut w = wolf_fighting(p(6.0, 0.0, 10.0))
        .kiting(HUNTER, p(6.0, 0.0, 10.0))
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    w.advance_clock(500_000);
    w = w.stopped(HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let effects = w.effects();
    assert_eq!(
        effects.len(),
        2,
        "one chase leg, then one stop: {effects:?}"
    );
    let (leg, stop) = (effects[0], effects[1]);
    assert!(
        near(leg.dest, p(6.0 - CONTACT_YD, 0.0, 10.0), 1e-4) && !leg.facing,
        "the chase leg must end at the contact point in front of the running player: {leg:?}"
    );
    assert!(
        stop.facing
            && stop.dur_ms == 0
            && stop.start == stop.dest
            && near(stop.dest, p(3.5, 0.0, 10.0), 0.01),
        "the chase leg stops where the client renders the creature: {stop:?}"
    );
    assert!(
        stop.spline_id > leg.spline_id,
        "the stop carries a newer spline id than the chase leg"
    );
    assert_eq!(
        (w.at(WOLF).at, w.at(WOLF).orientation),
        (stop.dest, 0.0),
        "server and client must agree on where the creature stopped and which way it now looks"
    );

    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        2,
        "the firing after the stop sends nothing"
    );
}

#[test]
fn every_chase_leg_travels_at_the_configured_run_speed() {
    let run = lyracore_shared::constants::speeds::RUN;
    for (case, twist) in [
        ("a standing victim", (|w| w) as Twist),
        ("a kiting victim", |w: Scenario| {
            w.kiting(HUNTER, p(15.0, 0.0, 10.0))
        }),
    ] {
        let mut w = twist(wolf_fighting(p(15.0, 0.0, 10.0)).attacking(WOLF, HUNTER));
        let tick = w.firing(SETTLED, 500_000, catch_all());
        run_cycle(&mut w, tick);

        let leg = w.effects();
        assert_eq!(leg.len(), 1, "one run leg on the covered firing ({case})");
        let travelled = ((leg[0].dest.x - leg[0].start.x).powi(2)
            + (leg[0].dest.y - leg[0].start.y).powi(2))
        .sqrt();
        let implied = travelled / (leg[0].dur_ms as f32 / 1000.0);
        assert!(
            leg[0].run && (implied - run).abs() < 0.05,
            "a leg travels at run speed: {travelled} yd in {} ms implies {implied} yd/s, not {run} ({case})",
            leg[0].dur_ms
        );
    }
}

#[test]
fn a_chase_leg_is_replaced_when_the_victim_comes_back_toward_the_creature() {
    let run = lyracore_shared::constants::speeds::RUN;
    let mut w = wolf_fighting(p(20.0, 0.0, 10.0))
        .kiting(HUNTER, p(20.0, 0.0, 10.0))
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    w.advance_clock(500_000);
    w = w.kiting(HUNTER, p(12.0, 0.0, 10.0)).stopped(HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let legs = w.effects();
    assert_eq!(
        legs.len(),
        2,
        "exactly one replacement leg this firing, appended to the first"
    );
    let replacement = legs[1];
    assert!(
        near(replacement.start, p(3.5, 0.0, 10.0), 0.01)
            && near(replacement.dest, p(12.0 - CONTACT_YD, 0.0, 10.0), 1e-4),
        "the replacement runs from the rendered position to the contact point: {replacement:?}"
    );
    let travelled = ((replacement.dest.x - replacement.start.x).powi(2)
        + (replacement.dest.y - replacement.start.y).powi(2))
    .sqrt();
    let implied = travelled / (replacement.dur_ms as f32 / 1000.0);
    assert!(
        (implied - run).abs() < 0.05,
        "the replacement travels at run speed: {travelled} yd in {} ms implies {implied} yd/s, not {run}",
        replacement.dur_ms
    );
    assert!(
        replacement.spline_id > legs[0].spline_id,
        "the replacement carries a newer spline id"
    );

    for _ in 0..8 {
        w.advance_clock(500_000);
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);
        assert!(
            w.at(WOLF).at.x <= 12.0 - CONTACT_YD + 1e-4,
            "the creature never passes the contact point, got x = {}",
            w.at(WOLF).at.x
        );
    }
    let last = w.effects().last().copied().expect("at least one effect");
    assert!(
        last.facing && last.dur_ms == 0 && last.start == last.dest && last.facing_angle == 0.0,
        "the fight settles on one facing spline: {last:?}"
    );
}

#[test]
fn a_creature_victim_with_a_leg_in_flight_is_chased_like_a_kiting_player() {
    let mut w = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .creature(PACK_MATE, p(4.0, 0.0, 10.0))
        .flying(
            PACK_MATE,
            p(4.0, 0.0, 10.0),
            p(4.0, 20.0, 10.0),
            SETTLED,
            LEG_MS,
        )
        .attacking(WOLF, PACK_MATE);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_running_legs_to(
        &w,
        &[p(4.0 - CONTACT_YD, 0.0, 10.0)],
        "a moving creature victim in melee reach is chased like a kiting player",
    );
}

#[test]
fn a_stopped_victim_outside_reach_with_no_leg_in_flight_still_gets_a_closing_leg() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0))
        .stopped(HUNTER)
        .attacking(WOLF, HUNTER);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_running_legs_to(
        &w,
        &[p(15.0 - CONTACT_YD, 0.0, 10.0)],
        "a standing victim out of reach without a leg in flight gets a closing leg",
    );
}

#[test]
fn an_offensive_caster_holds_at_its_spell_range() {
    let cases: [(&str, Twist, usize); 3] = [
        ("inside its range", |w| w, 0),
        (
            "its victim stepped out of range",
            |w| w.tweak_player(HUNTER, |hunter| hunter.at = p(30.0, 0.0, 10.0)),
            1,
        ),
        ("a wall between them", |w| w.wall_between(WOLF, HUNTER), 1),
    ];
    for (case, twist, legs) in cases {
        let mut w = twist(
            wolf_fighting(p(20.0, 0.0, 10.0))
                .caster(WOLF, 30.0)
                .attacking(WOLF, HUNTER),
        );
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            legs,
            "a caster holds at its range only with line of sight ({case})"
        );
    }
}

#[test]
fn a_caster_whose_victim_stops_inside_reach_turns_and_halts_its_leg() {
    let launched = SETTLED - 500_000;
    let mut w = wolf_fighting(p(7.0, 0.0, 10.0))
        .caster(WOLF, 30.0)
        .facing(WOLF, std::f32::consts::PI)
        .attacking(WOLF, HUNTER)
        .flying(
            WOLF,
            p(0.0, 0.0, 10.0),
            p(10.0, 0.0, 10.0),
            launched,
            LEG_MS,
        );
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let rendered = p(5.0, 0.0, 10.0);
    assert_eq!(
        w.effects()
            .iter()
            .map(|e| (e.start, e.dest, e.dur_ms, e.facing, e.facing_angle))
            .collect::<Vec<_>>(),
        [(rendered, rendered, 0, true, 0.0)],
        "a caster stops and faces a victim inside melee reach"
    );
    assert_eq!((w.at(WOLF).at, w.at(WOLF).orientation), (rendered, 0.0));
}

#[test]
fn a_crowd_controlled_creature_is_not_moved_by_chase() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .rooted(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty(),
        "a rooted creature does not slide toward its victim"
    );
}

#[test]
fn a_blocked_chase_goes_around_instead_of_through() {
    let mut w = wolf_fighting(p(20.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .detour(WOLF, (0.0, 20.0));
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects().first().map(|e| e.dest),
        Some(p(0.0, 20.0, 10.0)),
        "a chase leg follows the navigation detour"
    );
}

fn wolf_casting(player_at: Point) -> Scenario {
    wolf_fighting(player_at)
        .attacking(WOLF, HUNTER)
        .caster(WOLF, 30.0)
        .lone_spell(WOLF, NUKE)
}

#[test]
fn a_caster_at_spell_range_casts_and_stays_where_it_is() {
    let mut w = wolf_casting(p(20.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.casts(),
        [(WOLF, NUKE, HUNTER)],
        "a caster begins its cast in the firing it engages"
    );
    assert!(
        w.effects().is_empty() && w.victims() == [(WOLF, HUNTER)],
        "a casting firing throws no closing leg and keeps the engagement"
    );
}

#[test]
fn a_wall_blocked_caster_drops_the_spell_and_closes_instead() {
    let mut w = wolf_casting(p(20.0, 0.0, 10.0)).wall_between(WOLF, HUNTER);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.casts().is_empty() && w.effects().len() == 1,
        "a caster without line of sight closes instead of holding"
    );
}

#[test]
fn a_caster_that_cannot_act_or_is_already_casting_begins_nothing() {
    let held: [(&str, Twist); 2] = [
        ("stunned", |w| w.crowd_controlled(WOLF)),
        ("already casting", |w| w.mid_cast(WOLF)),
    ];
    for (case, hold) in held {
        let mut w = hold(wolf_casting(p(20.0, 0.0, 10.0)));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert!(
            w.casts().is_empty(),
            "a stun stops the cast and a running cast bar is not restarted ({case})"
        );
    }
}

#[test]
fn the_rotation_fires_its_highest_priority_ready_action() {
    let rotation = |w: Scenario| {
        w.rotation_line(WOLF, HEAL, CastWhen::Hurt(50), 10)
            .rotation_line(WOLF, NUKE, CastWhen::Always, 1)
    };
    let cases: [(&str, Twist, u32, u64); 3] = [
        ("unhurt, so the heal is not eligible", |w| w, NUKE, HUNTER),
        (
            "hurt, so the heal outranks the nuke",
            |w| w.hurt(WOLF, 10),
            HEAL,
            WOLF,
        ),
        (
            "hurt but the heal is on cooldown",
            |w| w.hurt(WOLF, 10).not_ready(HEAL),
            NUKE,
            HUNTER,
        ),
    ];
    for (case, twist, spell_id, target) in cases {
        let mut w = twist(rotation(wolf_casting(p(20.0, 0.0, 10.0))));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.casts(),
            [(WOLF, spell_id, target)],
            "the rotation fires its highest-priority ready line ({case})"
        );
    }
}

#[test]
fn a_missing_aura_line_waits_until_the_aura_is_actually_missing() {
    let cases: [(&str, Twist, Vec<CastEffect>); 4] = [
        (
            "the victim lacks the debuff",
            |w| w,
            vec![(WOLF, DEBUFF, HUNTER)],
        ),
        (
            "the victim already has it",
            |w| w.carrying(HUNTER, DEBUFF),
            vec![],
        ),
        (
            "the buff has lapsed",
            |w| {
                w.carrying(HUNTER, DEBUFF)
                    .rotation_line(WOLF, BUFF, CastWhen::SelfLacksIt, 1)
            },
            vec![(WOLF, BUFF, WOLF)],
        ),
        (
            "it is already buffed",
            |w| {
                w.carrying(HUNTER, DEBUFF)
                    .rotation_line(WOLF, BUFF, CastWhen::SelfLacksIt, 1)
                    .carrying(WOLF, BUFF)
            },
            vec![],
        ),
    ];
    for (case, twist, casts) in cases {
        let mut w = twist(wolf_casting(p(20.0, 0.0, 10.0)).rotation_line(
            WOLF,
            DEBUFF,
            CastWhen::VictimLacksIt,
            5,
        ));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.casts(),
            casts.as_slice(),
            "a running debuff is not reapplied and a lapsed buff is ({case})"
        );
    }
}

#[test]
fn only_a_creature_with_no_rotation_at_all_falls_back_to_its_one_spell() {
    let cases: [(&str, Twist, Vec<CastEffect>); 3] = [
        ("no rotation", |w| w, vec![(WOLF, NUKE, HUNTER)]),
        (
            "a rotation wins over the lone spell",
            |w| w.rotation_line(WOLF, DEBUFF, CastWhen::Always, 1),
            vec![(WOLF, DEBUFF, HUNTER)],
        ),
        (
            "a rotation this server cannot read is still a rotation",
            |w| w.rotation_line(WOLF, DEBUFF, CastWhen::Never, 1),
            vec![],
        ),
    ];
    for (case, twist, casts) in cases {
        let mut w = twist(wolf_casting(p(20.0, 0.0, 10.0)));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.casts(),
            casts.as_slice(),
            "single-spell casters keep casting; an unknown rotation does not fall back ({case})"
        );
    }
}

#[test]
fn a_creature_re_points_at_a_stronger_threat_and_chases_it_in_the_same_cycle() {
    let mut w = wolf_fighting(p(5.0, 0.0, 10.0))
        .player(RANGER, p(20.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .threat(WOLF, HUNTER, 50)
        .threat(WOLF, RANGER, 100);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.victims(),
        [(WOLF, RANGER)],
        "a creature switches to whoever out-threats its target"
    );
    assert_running_legs_to(
        &w,
        &[p(20.0 - CONTACT_YD, 0.0, 10.0)],
        "the retarget lands before the movement decision",
    );
}

#[test]
fn equal_threat_leaves_the_target_where_it_is() {
    let mut w = wolf_fighting(p(20.0, 0.0, 10.0))
        .player(RANGER, p(5.0, 0.0, 10.0))
        .attacking(WOLF, RANGER)
        .threat(WOLF, HUNTER, 50)
        .threat(WOLF, RANGER, 50);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.victims(),
        [(WOLF, RANGER)],
        "a switch needs strictly more threat"
    );
}

#[test]
fn a_taunt_pins_the_target_whatever_the_threat_table_says() {
    let cases: [(&str, Twist, u64); 2] = [
        (
            "pinned on the weaker source",
            |w| w.taunted(WOLF, HUNTER),
            HUNTER,
        ),
        (
            "yanked onto the taunter",
            |w| w.taunted(WOLF, RANGER),
            RANGER,
        ),
    ];
    for (case, taunt, victim) in cases {
        let mut w = taunt(
            wolf_fighting(p(5.0, 0.0, 10.0))
                .player(RANGER, p(20.0, 0.0, 10.0))
                .attacking(WOLF, HUNTER)
                .threat(WOLF, HUNTER, 50)
                .threat(WOLF, RANGER, 100),
        );
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.victims(),
            [(WOLF, victim)],
            "taunt forces the target until it ends ({case})"
        );
    }
}

fn wolf_cornered() -> Scenario {
    wolf_fighting(p(3.0, 0.0, 10.0)).attacking(WOLF, HUNTER)
}

fn healthy_rout_ms() -> u32 {
    (FLEE_LEG_YD / lyracore_shared::constants::speeds::RUN * 1000.0) as u32
}

#[test]
fn a_wounded_runner_breaks_off_and_sprints_away_from_its_victim() {
    let mut w = wolf_cornered().wounded_runner(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let now_ms = (SETTLED / 1000) as u32;
    let leg = w.effects();
    assert_eq!(leg.len(), 1, "the rout is the router's SOLE mover");
    assert_eq!(
        (leg[0].dest, leg[0].run),
        (p(-FLEE_LEG_YD, 0.0, 10.0), true),
        "a rout runs the full committed dash away from the foe"
    );
    assert!(
        leg[0].dur_ms > healthy_rout_ms(),
        "a wounded runner is slower than a healthy one"
    );
    assert_eq!(
        w.rout_ends_ms(WOLF),
        now_ms + ROUT_DURATION_MS,
        "the rout must be BOUNDED — an unstamped window is a creature that runs until it dies"
    );
    assert!(
        w.flagged().contains(&WOLF) && w.flagged().contains(&HUNTER),
        "a rout re-stamps combat on both sides"
    );
}

#[test]
fn a_creature_that_fights_to_the_death_never_enters_the_rout_path() {
    let mut w = wolf_cornered().hurt(WOLF, 10);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty() && w.rout_ends_ms(WOLF) == 0,
        "only flee-eligible creatures rout"
    );
}

#[test]
fn a_router_rides_its_committed_leg_and_re_rolls_only_once_it_ends() {
    let mut w = wolf_cornered().wounded_runner(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    let dur_ms = w.effects()[0].dur_ms;

    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(w.effects().len(), 1, "the committed rout leg plays out");
    assert_eq!(
        w.victims(),
        [(WOLF, HUNTER)],
        "a router must stay ENGAGED while it runs, or it can be neither chased down nor killed"
    );

    w.advance_clock(dur_ms as u64 * 1000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.effects().len(),
        2,
        "a rout whose leg ended mid-window keeps running"
    );
}

#[test]
fn a_spent_rout_returns_the_creature_to_chasing_and_never_opens_a_second() {
    let mut w = wolf_cornered().wounded_runner(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);
    let opened_at = w.rout_ends_ms(WOLF);

    w.advance_clock(ROUT_DURATION_MS as u64 * 1000 + 1_000_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let chase = w.effects()[1];
    assert!(
        chase.dest.x > chase.start.x,
        "after the window the creature closes on its victim"
    );
    assert_eq!(
        w.rout_ends_ms(WOLF),
        opened_at,
        "the spent rout clock is not re-stamped"
    );
}

#[test]
fn a_feared_creature_is_moved_by_fear_and_by_nothing_else() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .feared_by(WOLF, HUNTER)
        .rolls([u32::MAX / 2]);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    let leg = w.effects();
    assert_eq!(leg.len(), 1, "other movers skip a feared creature");
    assert!(
        leg[0].dest.x < -14.0 && leg[0].run,
        "a feared creature is force-walked AWAY from whoever feared it, at a run; got {:?}",
        leg[0].dest
    );
    assert_eq!(w.victims(), [(WOLF, HUNTER)], "fear does not disengage");
}

#[test]
fn two_fear_sources_panic_the_creature_once() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .feared_by(WOLF, HUNTER)
        .feared_by(WOLF, RANGER)
        .rolls([u32::MAX / 2]);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(w.effects().len(), 1, "several fear auras make one leg");
}

#[test]
fn fear_owns_a_wounded_runner_and_leaves_its_rout_window_intact() {
    let cases: [(&str, Twist, bool); 2] = [
        ("about to rout", |w| w.wounded_runner(WOLF), false),
        ("already routing", |w| w.routing(WOLF), true),
    ];
    for (case, twist, was_open) in cases {
        let mut w = twist(
            wolf_fighting(p(15.0, 0.0, 10.0))
                .attacking(WOLF, HUNTER)
                .rolls([u32::MAX / 2]),
        )
        .feared_by(WOLF, HUNTER);
        let before = w.rout_ends_ms(WOLF);
        let tick = w.tick(false, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(w.effects().len(), 1, "no rout leg while feared ({case})");
        assert_eq!(
            (w.rout_ends_ms(WOLF), before != 0),
            (before, was_open),
            "fear leaves the rout window unchanged ({case})"
        );
    }
}

#[test]
fn a_crowd_controlled_router_keeps_its_state_and_resumes_when_the_control_ends() {
    let mut w = wolf_cornered().routing(WOLF).rooted(WOLF);
    let open_until = w.rout_ends_ms(WOLF);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.effects().is_empty(),
        "a frozen creature must not move at all — not by chase, not by its own rout"
    );
    assert_eq!(
        (w.victims(), w.rout_ends_ms(WOLF)),
        (vec![(WOLF, HUNTER)], open_until),
        "crowd control keeps the engagement and the rout window"
    );

    w.frozen.borrow_mut().remove(&WOLF);
    w.advance_clock(500_000);
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects().len(),
        1,
        "the creature must resume its rout on the very next firing, inside the window it kept"
    );
}

fn warlock_and_imp(imp_at: Point) -> Scenario {
    Scenario::new(SETTLED)
        .player(HUNTER, p(0.0, 0.0, 10.0))
        .pet(IMP, HUNTER, imp_at)
}

#[test]
fn a_pet_takes_its_owners_foe_and_closes_on_it_in_the_same_cycle() {
    let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
        .creature(WOLF, p(20.0, 0.0, 10.0))
        .attacking(HUNTER, WOLF);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pet_effects(),
        [PetEffect::Took(IMP, WOLF)],
        "a pet engages what its owner fights"
    );
    assert_eq!(
        w.effects().iter().map(|e| e.guid).collect::<Vec<_>>(),
        [IMP]
    );
    assert_running_legs_to(
        &w,
        &[p(20.0 - CONTACT_YD, 0.0, 10.0)],
        "a newly armed pet closes in the same firing",
    );
}

#[test]
fn only_a_pet_left_off_passive_assists_its_owner() {
    for (react, assists) in [
        (PetReact::Passive, false),
        (PetReact::Defensive, true),
        (PetReact::Aggressive, true),
    ] {
        let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
            .creature(WOLF, p(20.0, 0.0, 10.0))
            .attacking(HUNTER, WOLF)
            .pet_bar(HUNTER, PetOrder::Follow, react);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.pet_effects() == [PetEffect::Took(IMP, WOLF)],
            assists,
            "a PASSIVE pet does not join ({react:?})"
        );
    }
}

#[test]
fn only_an_aggressive_pet_seeks_a_foe_its_owner_is_not_fighting() {
    for (react, seeks) in [(PetReact::Defensive, false), (PetReact::Aggressive, true)] {
        let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
            .creature(WOLF, p(6.0, 0.0, 10.0))
            .pet_bar(HUNTER, PetOrder::Follow, react);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.pet_effects() == [PetEffect::Took(IMP, WOLF)],
            seeks,
            "only an AGGRESSIVE pet starts a fight ({react:?})"
        );
    }
}

#[test]
fn an_attack_order_wins_over_the_owners_fight_and_a_stale_one_is_dropped() {
    let commanded = || {
        warlock_and_imp(p(0.0, 0.0, 10.0))
            .creature(WOLF, p(20.0, 0.0, 10.0))
            .creature(PACK_MATE, p(-20.0, 0.0, 10.0))
            .attacking(HUNTER, WOLF)
            .pet_bar(HUNTER, PetOrder::Attack(PACK_MATE), PetReact::Defensive)
    };

    let mut w = commanded();
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.pet_effects(),
        [PetEffect::Took(IMP, PACK_MATE)],
        "the attack order overrides assist"
    );

    let mut w = commanded().slain(PACK_MATE);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.pet_effects(),
        [PetEffect::OrderCleared(HUNTER), PetEffect::Took(IMP, WOLF)],
        "an order whose foe died is dropped"
    );
}

#[test]
fn a_pet_that_drifted_out_of_the_follow_band_runs_back_and_one_inside_it_holds() {
    for (imp_at, legs) in [(p(-20.0, 0.0, 10.0), 1), (p(-7.0, 0.0, 10.0), 0)] {
        let mut w = warlock_and_imp(imp_at);
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.effects().len(),
            legs,
            "no follow leg inside the follow band ({imp_at:?})"
        );
    }

    let mut w = warlock_and_imp(p(-20.0, 0.0, 10.0));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    let leg = w.effects()[0];
    assert_eq!(
        (leg.dest, leg.run),
        (p(-4.0, 0.0, 10.0), true),
        "the pet runs back and stops short of its owner"
    );
    assert_eq!(
        leg.dur_ms,
        (16.0 / lyracore_shared::constants::speeds::RUN * 1000.0) as u32,
        "the follow leg spans the sense period"
    );
}

#[test]
fn a_pet_whose_owner_is_gone_or_dead_despawns_and_so_does_its_own_corpse() {
    let cases: [(&str, Twist); 2] = [
        ("the owner died", |w| w.corpse(HUNTER)),
        ("the pet was killed", |w| w.slain(IMP)),
    ];
    for (case, twist) in cases {
        let mut w = twist(warlock_and_imp(p(0.0, 0.0, 10.0)));
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.pet_effects(),
            [PetEffect::Dismissed(IMP)],
            "a pet without a live owner is removed ({case})"
        );
    }

    let mut orphan = Scenario::new(SETTLED).pet(IMP, HUNTER, p(0.0, 0.0, 10.0));
    let tick = orphan.tick(true, catch_all());
    run_cycle(&mut orphan, tick);
    assert_eq!(
        orphan.pet_effects(),
        [PetEffect::Dismissed(IMP)],
        "an owner who logged out leaves no row at all, and their pet must go with them"
    );
}

#[test]
fn a_frozen_pet_holds_still_and_a_feared_one_is_moved_by_fear_alone() {
    let mut rooted = warlock_and_imp(p(-20.0, 0.0, 10.0)).rooted(IMP);
    let tick = rooted.tick(true, catch_all());
    run_cycle(&mut rooted, tick);
    assert!(
        rooted.effects().is_empty() && rooted.pet_effects().is_empty(),
        "a rooted pet does not follow its owner"
    );

    let mut feared = warlock_and_imp(p(-20.0, 0.0, 10.0))
        .feared_by(IMP, HUNTER)
        .rolls([u32::MAX / 2]);
    let tick = feared.tick(true, catch_all());
    run_cycle(&mut feared, tick);
    assert!(
        feared.pet_effects().is_empty() && feared.effects().len() == 1,
        "fear is the feared pet's only mover"
    );
    assert!(
        feared.effects()[0].dest.x < -20.0,
        "the surviving leg is the fear dash away from the warlock; got {:?}",
        feared.effects()[0].dest
    );
}

#[test]
fn a_pet_told_to_stay_stops_trailing_its_owner_but_still_fights() {
    let stay = |w: Scenario| w.pet_bar(HUNTER, PetOrder::Stay, PetReact::Defensive);

    let mut w = stay(warlock_and_imp(p(-20.0, 0.0, 10.0)));
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    assert!(w.effects().is_empty(), "a STAY pet does not follow");

    let mut w = stay(
        warlock_and_imp(p(-20.0, 0.0, 10.0))
            .creature(WOLF, p(20.0, 0.0, 10.0))
            .attacking(HUNTER, WOLF),
    );
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);
    assert_eq!(
        w.pet_effects(),
        [PetEffect::Took(IMP, WOLF)],
        "a STAY pet still engages"
    );
}

#[test]
fn a_pet_whose_owner_stopped_fighting_drops_its_own_attack() {
    let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
        .creature(WOLF, p(20.0, 0.0, 10.0))
        .attacking(IMP, WOLF);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pet_effects(),
        [PetEffect::StoodDown(IMP)],
        "a pet left behind by its owner drops its fight"
    );
    assert!(
        w.effects().is_empty() && w.victims().is_empty(),
        "a pet that stood down this firing must not also be chased into the fight it just left"
    );
}

#[test]
fn a_wounded_pet_armed_this_firing_closes_instead_of_routing() {
    let mut w = warlock_and_imp(p(0.0, 0.0, 10.0))
        .hurt(IMP, 10)
        .creature(WOLF, p(20.0, 0.0, 10.0))
        .attacking(HUNTER, WOLF);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let leg = w.effects();
    assert_eq!(leg.len(), 1, "one leg per firing, and it is the chase's");
    assert!(
        leg[0].dest.x > 0.0 && w.rout_ends_ms(IMP) == 0,
        "a pet does not rout"
    );
}

#[test]
fn a_pet_whose_owner_changed_instance_is_snapped_across_rather_than_walked() {
    let mut w = warlock_and_imp(p(-20.0, 0.0, 10.0)).in_instance(IMP, 7);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.pet_effects(),
        [PetEffect::Restaged(IMP, p(0.0, 0.0, 10.0), MAP, INSTANCE)],
        "a pet left on another map or instance is re-placed"
    );
    assert!(
        w.effects().is_empty(),
        "the snap must relay no leg; the AOI create/destroy carries the pet across"
    );
    let imp = w.at(IMP);
    let grid = spatial::grid_cell(0.0, 0.0);
    assert_eq!(
        (imp.at, imp.grid, imp.cell),
        (
            p(0.0, 0.0, 10.0),
            grid,
            spatial::grid_cell_id(grid.0, grid.1)
        ),
        "position, grid address and packed cell move together"
    );
}

const SETTLED_MS: u64 = SETTLED / 1000;

#[test]
fn a_creature_that_engaged_and_chased_this_cycle_is_not_healed_by_the_same_cycle() {
    let mut w = wolf_fighting(p(15.0, 0.0, 10.0))
        .awake([WOLF])
        .hurt(WOLF, 10);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        !w.pulls().is_empty() && !w.effects().is_empty(),
        "the wolf pulled and closed on the player this firing"
    );
    assert_eq!(
        w.at(WOLF).health,
        10,
        "regeneration runs after chase and skips the engaged chaser"
    );
}

#[test]
fn an_out_of_combat_unit_recovers_both_bars_and_is_not_moved_by_it() {
    let mut w = Scenario::new(HALF_WAY)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .awake([WOLF])
        .hurt(WOLF, 10)
        .power(WOLF, 20, 100);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    let wolf = w.at(WOLF);
    assert_eq!(
        (wolf.health, wolf.power),
        (15, 25),
        "an out-of-combat unit recovers both bars every sense firing"
    );
    assert!(
        w.effects().is_empty() && wolf.at == p(0.0, 0.0, 10.0),
        "regeneration writes health and power only"
    );
}

#[test]
fn a_unit_in_combat_heals_only_through_a_combat_regen_aura() {
    let cases: [(&str, Twist, u32); 2] = [
        ("no aura, the vanilla default", |w| w, 10),
        ("carrying Troll Regeneration", |w| w.combat_regen(WOLF), 11),
    ];
    for (case, twist, health) in cases {
        let mut w = twist(
            Scenario::new(SETTLED)
                .creature(WOLF, p(0.0, 0.0, 10.0))
                .hurt(WOLF, 10)
                .in_combat_until(WOLF, SETTLED_MS + 1000),
        );
        let tick = w.tick(true, catch_all());
        run_cycle(&mut w, tick);

        assert_eq!(
            w.at(WOLF).health,
            health,
            "combat stops regeneration except from auras ({case})"
        );
    }
}

#[test]
fn a_unit_past_its_combat_deadline_loses_the_flag_and_one_inside_it_keeps_it() {
    let mut w = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .in_combat_until(WOLF, SETTLED_MS - 1000)
        .creature(PACK_MATE, p(5.0, 0.0, 10.0))
        .in_combat_until(PACK_MATE, SETTLED_MS + 1000);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.unflagged(),
        [WOLF],
        "the combat flag lifts about six seconds after the last hostile action"
    );
    assert!(
        !w.in_combat(WOLF) && w.in_combat(PACK_MATE),
        "only the expired deadline may clear"
    );
}

#[test]
fn combat_entry_outlasts_combat_exit_in_the_same_cycle() {
    let mut w = wolf_fighting(p(5.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .in_combat_until(WOLF, SETTLED_MS - 1000);
    let tick = w.tick(true, catch_all());
    run_cycle(&mut w, tick);

    assert!(
        w.unflagged().is_empty() && w.in_combat(WOLF),
        "exit clears only a deadline already past"
    );
}

#[test]
fn a_creature_the_leash_dropped_walks_home_and_leaves_combat_in_one_cycle() {
    let mut evaded = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 20.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), false)
        .hurt(WOLF, 10)
        .in_combat_until(WOLF, SETTLED_MS - 1000);
    let tick = evaded.tick(true, catch_all());
    run_cycle(&mut evaded, tick);

    let legs = evaded.effects();
    assert_eq!(legs.len(), 1, "one leg per firing, and it is the walk home");
    assert!(
        legs[0].dest.y < 20.0 && legs[0].run,
        "a released creature runs back to its post; got {:?}",
        legs[0].dest
    );
    assert_eq!(
        (evaded.unflagged(), evaded.at(WOLF).health),
        (vec![WOLF], 10),
        "recovery starts the firing after the flag lifts"
    );

    let mut pursuing = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 20.0, 10.0))
        .awake([WOLF])
        .home(WOLF, p(0.0, 0.0, 10.0), false)
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(0.0, 40.0, 10.0))
        .attacking(WOLF, HUNTER)
        .in_combat_until(WOLF, SETTLED_MS - 1000);
    let tick = pursuing.tick(true, catch_all());
    run_cycle(&mut pursuing, tick);

    let legs = pursuing.effects();
    assert_eq!(legs.len(), 1, "one leg per firing, and it is the chase");
    assert!(
        legs[0].dest.y > 20.0 && pursuing.in_combat(WOLF),
        "a live pursuit keeps the creature engaged; got {:?}",
        legs[0].dest
    );
}

#[test]
fn regeneration_and_combat_exit_run_only_on_the_catch_all_sense_firing() {
    let cases: [(&str, bool, TickScope, u32, Vec<u64>); 3] = [
        (
            "the catch-all sense firing",
            true,
            catch_all(),
            15,
            vec![WOLF],
        ),
        ("a movement-only firing", false, catch_all(), 10, vec![]),
        (
            "a dedicated instance firing",
            true,
            TickScope::Only(7),
            10,
            vec![],
        ),
    ];
    for (case, sense, scope, health, unflagged) in cases {
        let mut w = Scenario::new(SETTLED)
            .creature(WOLF, p(5.0, 0.0, 10.0))
            .in_combat_until(WOLF, SETTLED_MS - 1000)
            .creature(PACK_MATE, p(0.0, 0.0, 10.0))
            .hurt(PACK_MATE, 10);
        let tick = w.tick(sense, scope);
        run_cycle(&mut w, tick);

        assert_eq!(
            (w.at(PACK_MATE).health, w.unflagged()),
            (health, unflagged),
            "only the catch-all row regenerates and sweeps deadlines ({case})"
        );
    }
}

const DEDICATED: u64 = 7;
const WARDEN: u64 = RANGER + 1;

fn one_fight_per_instance() -> Scenario {
    Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .creature(PACK_MATE, p(0.0, 0.0, 10.0))
        .in_instance(PACK_MATE, DEDICATED)
        .creature(FAR_MATE, p(0.0, 0.0, 10.0))
        .in_instance(FAR_MATE, 9)
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(15.0, 0.0, 10.0))
        .player(RANGER, p(15.0, 0.0, 10.0))
        .player(WARDEN, p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .attacking(PACK_MATE, RANGER)
        .attacking(FAR_MATE, WARDEN)
}

#[test]
fn the_catch_all_firing_covers_every_instance_exactly_once() {
    let mut w = one_fight_per_instance();
    let tick = w.tick(false, catch_all());
    run_cycle(&mut w, tick);

    assert_eq!(
        w.effects().iter().map(|e| e.guid).collect::<Vec<_>>(),
        [WOLF, PACK_MATE, FAR_MATE],
        "without a dedicated row the seeded row ticks every creature once"
    );
}

#[test]
fn a_dedicated_row_and_the_catch_all_stay_a_strict_partition() {
    let mut w = one_fight_per_instance();
    let tick = w.tick(
        false,
        TickScope::from_rows(
            crate::creatures::GLOBAL_TICK_INSTANCE,
            [crate::creatures::GLOBAL_TICK_INSTANCE, DEDICATED],
        ),
    );
    run_cycle(&mut w, tick);
    let after_catch_all = w.effects().iter().map(|e| e.guid).collect::<Vec<_>>();

    let tick = w.tick(false, TickScope::Only(DEDICATED));
    run_cycle(&mut w, tick);

    assert_eq!(
        after_catch_all,
        [WOLF, FAR_MATE],
        "the catch-all skips a dedicated instance"
    );
    assert_eq!(
        w.effects().iter().map(|e| e.guid).collect::<Vec<_>>(),
        [WOLF, FAR_MATE, PACK_MATE],
        "the two firings move every creature exactly once"
    );
    let mut flagged = w.flagged();
    flagged.sort_unstable();
    let mut once = flagged.clone();
    once.dedup();
    assert_eq!(flagged, once, "each unit is flagged by one row");
}

#[test]
fn movement_runs_every_firing_while_sensing_holds_the_four_second_cadence() {
    let mut settled_at = Vec::new();
    for (interval_micros, firings) in [(100_000i64, 41u64), (crate::creatures::MOVE_TICK_MICROS, 9)]
    {
        let mut w = Scenario::new(0).creature(WOLF, p(0.0, 0.0, 10.0)).flying(
            WOLF,
            p(0.0, 0.0, 10.0),
            p(60.0, 0.0, 10.0),
            0,
            60_000,
        );
        let mut moved = 0u64;
        let mut sensed = 0u64;
        for k in 0..firings {
            let tick = w.firing(k * interval_micros as u64, interval_micros, catch_all());
            let outcome = run_cycle(&mut w, tick);
            moved += outcome.rows_visited[0].1;
            sensed += u64::from(
                outcome
                    .rows_visited
                    .iter()
                    .any(|(pass, _)| *pass == "aggro"),
            );
        }
        assert_eq!(
            (moved, u64::from(w.maintenance_runs.get()), sensed),
            (firings, firings, 2),
            "movement and due-time maintenance follow each firing; sensing stays at four seconds"
        );
        settled_at.push(w.at(WOLF).at);
    }
    assert_eq!(
        settled_at[0], settled_at[1],
        "movement per world second is independent of cadence"
    );
}

#[test]
fn discovery_stays_on_the_narrow_candidate_universes() {
    let mut w = Scenario::new(SETTLED)
        .creature(WOLF, p(0.0, 0.0, 10.0))
        .creature(PACK_MATE, p(60.0, 0.0, 10.0))
        .at_war(BEASTS, ALLIANCE)
        .player(HUNTER, p(15.0, 0.0, 10.0))
        .attacking(WOLF, HUNTER)
        .awake([WOLF, PACK_MATE]);
    for i in 0..10 {
        w = w.creature(FAR_MATE + i, p(500.0 + i as f32, 0.0, 10.0));
    }
    w = w.hurt(FAR_MATE, 10);
    let population = w.creatures.borrow().len();

    let tick = w.tick(true, catch_all());
    let outcome = run_cycle(&mut w, tick);

    assert_eq!(
        population, 12,
        "the world is larger than any phase's candidate set"
    );
    assert_eq!(
        (outcome.awake, outcome.rows_visited),
        (
            2,
            vec![
                ("advance", 0),
                ("patrol", 0),
                ("aggro", 1),
                ("assist", 0),
                ("pet", 0),
                ("eventai", 0),
                ("cast", 1),
                ("threat_retarget", 1),
                ("chase", 1),
                ("combat_enter", 1),
                ("idle", 2),
                ("regen*", 1),
                ("combat_drop*", 0),
                ("rout", 1),
                ("fear", 0),
            ]
        ),
        "work scales with fights and awake creatures, not the population of {population}"
    );
}
