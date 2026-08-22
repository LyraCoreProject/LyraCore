//! Engaged EventAI scenarios.
use super::*;
use crate::creatures::eventai::*;

const CREATURE: u64 = 8_101;
const TARGET: u64 = 8_102;
const FRIEND: u64 = 8_103;
const SECOND_TARGET: u64 = 8_104;
const ENTRY: u32 = 902;

fn point(x: f32) -> Point {
    Point { x, y: 0.0, z: 0.0 }
}

fn scope() -> TickScope {
    TickScope::CatchAll {
        dedicated: HashSet::new(),
    }
}

fn fire(scenario: &mut Scenario) {
    let tick = scenario.tick(true, scope());
    run_cycle(scenario, tick);
}

#[expect(
    clippy::too_many_arguments,
    reason = "test rows keep each authored value at the call site"
)]
fn row(
    id: u64,
    rule_id: u64,
    order: u8,
    event_type: u8,
    action_type: u8,
    repeat_policy: u8,
    event_params: [u32; 6],
    action_params: [u32; 3],
) -> CreatureAiEvent {
    CreatureAiEvent {
        id,
        creature_entry: ENTRY,
        event_type,
        action_type,
        text: String::new(),
        spell_id: 0,
        initial_min_ms: 0,
        initial_max_ms: 0,
        repeat_min_ms: 0,
        repeat_max_ms: 0,
        source_rule_id: rule_id,
        action_order: order,
        creature_guid: 0,
        chance_pct: 100,
        allowed_phase_mask: 1,
        source_flags: 0,
        repeat_policy,
        event_param_1: event_params[0],
        event_param_2: event_params[1],
        event_param_3: event_params[2],
        event_param_4: event_params[3],
        event_param_5: event_params[4],
        event_param_6: event_params[5],
        action_param_1: action_params[0],
        action_param_2: action_params[1],
        action_param_3: action_params[2],
        target_policy: TARGET_CURRENT,
        cast_options: 0,
    }
}

fn world() -> Scenario {
    Scenario::new(0)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(3.0))
        .at_war(BEASTS, ALLIANCE)
        .attacking(CREATURE, TARGET)
}

fn condition_rule(source_rule_id: u64, event: EventCondition) -> EventAiRule {
    EventAiRule {
        source_rule_id,
        event,
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions: Vec::new(),
    }
}

fn aggro_threat_definition(instructions: Vec<CreatureInstruction>) -> EventAiDefinition {
    let rule = EventAiRule {
        source_rule_id: 509_0000,
        event: EventCondition::OnAggro,
        chance_pct: 100,
        allowed_phases: PhaseSet { bits: u32::MAX },
        recurrence: RecurrencePolicy::Once,
        selection: InstructionSelection::All,
        execution: ExecutionPolicy::Ordinary,
        posture: PostureAdmission::Any,
        instructions,
    };
    let subject = EventAiSubject::Entry(ENTRY);
    EventAiDefinition {
        subject,
        revision: normalized_revision(subject, std::slice::from_ref(&rule)),
        rules: vec![rule],
    }
}

fn dispatch_aggro(scenario: &mut Scenario, current_target_guid: Option<u64>) {
    evaluate(
        scenario,
        EventAiRequest::Edge(EventContext {
            current_target_guid,
            engaged: true,
            ..EventContext::empty(EventKind::OnAggro, CREATURE, 0)
        }),
    );
}

#[test]
fn percent_threat_instructions_apply_in_source_order_and_keep_zero_rows() {
    let definition = aggro_threat_definition(vec![
        CreatureInstruction::ScaleSelectedThreat(ScaleSelectedThreatInstruction {
            percent: -50,
            target: InstructionTarget::CurrentOpponent,
        }),
        CreatureInstruction::ScaleSelectedThreat(ScaleSelectedThreatInstruction {
            percent: 50,
            target: InstructionTarget::CurrentOpponent,
        }),
    ]);
    let mut scenario = world()
        .threat(CREATURE, TARGET, 100)
        .eventai_native_definition(definition);

    dispatch_aggro(&mut scenario, Some(TARGET));

    assert_eq!(scenario.threat_value(CREATURE, TARGET), Some(75));

    let definition = aggro_threat_definition(vec![CreatureInstruction::ScaleAllThreat(
        ScaleAllThreatInstruction { percent: -100 },
    )]);
    let mut scenario = world()
        .threat(CREATURE, TARGET, 5)
        .eventai_native_definition(definition);

    dispatch_aggro(&mut scenario, Some(TARGET));

    assert_eq!(scenario.threat_value(CREATURE, TARGET), Some(0));
}

#[test]
fn absent_selected_threat_target_refuses_without_fallback() {
    let definition = aggro_threat_definition(vec![CreatureInstruction::ScaleSelectedThreat(
        ScaleSelectedThreatInstruction {
            percent: -50,
            target: InstructionTarget::Invoker,
        },
    )]);
    let mut scenario = world()
        .threat(CREATURE, TARGET, 100)
        .eventai_native_definition(definition);

    dispatch_aggro(&mut scenario, Some(TARGET));

    assert_eq!(scenario.threat_value(CREATURE, TARGET), Some(100));
}

#[test]
fn threat_scale_retargets_by_lowest_guid_but_preserves_a_taunt_lock() {
    let definition = aggro_threat_definition(vec![CreatureInstruction::ScaleSelectedThreat(
        ScaleSelectedThreatInstruction {
            percent: -50,
            target: InstructionTarget::CurrentOpponent,
        },
    )]);
    let higher_guid = SECOND_TARGET + 1;
    let mut scenario = world()
        .player(SECOND_TARGET, point(6.0))
        .player(higher_guid, point(9.0))
        .threat(CREATURE, TARGET, 100)
        .threat(CREATURE, SECOND_TARGET, 100)
        .threat(CREATURE, higher_guid, 100)
        .eventai_native_definition(definition);

    dispatch_aggro(&mut scenario, Some(TARGET));
    fire(&mut scenario);

    assert_eq!(scenario.threat_value(CREATURE, TARGET), Some(50));
    assert_eq!(scenario.victims(), vec![(CREATURE, SECOND_TARGET)]);

    let definition = aggro_threat_definition(vec![CreatureInstruction::ScaleSelectedThreat(
        ScaleSelectedThreatInstruction {
            percent: -50,
            target: InstructionTarget::CurrentOpponent,
        },
    )]);
    let mut scenario = world()
        .player(SECOND_TARGET, point(6.0))
        .threat(CREATURE, TARGET, 100)
        .threat(CREATURE, SECOND_TARGET, 100)
        .taunted(CREATURE, TARGET)
        .eventai_native_definition(definition);

    dispatch_aggro(&mut scenario, Some(TARGET));
    fire(&mut scenario);

    assert_eq!(scenario.victims(), vec![(CREATURE, TARGET)]);
}

#[test]
fn main_spell_metadata_reaches_the_authored_posture_seam() {
    let mut rule = condition_rule(
        509_0001,
        EventCondition::TimedGeneric(TimeWindow {
            min_ms: 0,
            max_ms: 0,
        }),
    );
    rule.instructions = vec![CreatureInstruction::Cast(CastInstruction {
        spell_id: 501,
        target: InstructionTarget::CurrentOpponent,
        interrupt_previous: false,
        start_mode: SpellStartMode::Direct,
        caster_role: SpellCasterRole::Actor,
        target_role: SpellTargetRole::Selected,
        aura_absent: false,
        character_only: false,
        target_must_be_casting: false,
        main_spell: true,
        distance_after_start: false,
    })];
    let subject = EventAiSubject::Entry(ENTRY);
    let definition = EventAiDefinition {
        subject,
        revision: normalized_revision(subject, std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let scenario = world().eventai_native_definition(definition);

    assert_eq!(
        scenario.authored_combat(CREATURE),
        AuthoredCombat {
            casting: true,
            main_spell_posture: true,
            flee: false,
        }
    );
}

#[test]
fn a_false_hp_condition_does_not_consume_a_roll_or_create_rule_state() {
    let mut scenario = world().eventai_row(row(
        509_0100,
        509_0100,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT,
        [0, 10, 1_000, 1_000, 0, 0],
        [100, 0, 0],
    ));

    fire(&mut scenario);

    assert!(scenario.casts().is_empty());
    assert!(scenario.eventai_state(CREATURE, 509_0100).is_none());
}

#[test]
fn inclusive_hp_endpoints_rearm_a_continuously_true_rule_after_its_cooldown() {
    let mut scenario = world()
        .hurt(CREATURE, 50)
        .eventai_broadcast(1, "at half", 0)
        .eventai_row(row(
            509_0101,
            509_0101,
            0,
            EVENT_CREATURE_HP,
            ACTION_SAY,
            REPEAT,
            [50, 50, 1_000, 1_000, 0, 0],
            [1, 0, 0],
        ));

    fire(&mut scenario);
    fire(&mut scenario);
    scenario.advance_clock(1_000_000);
    fire(&mut scenario);

    assert_eq!(scenario.eventai_speech().len(), 2);
}

#[test]
fn percentage_windows_compare_the_source_integer_percentage() {
    let mut scenario = world()
        .tweak(CREATURE, |creature| {
            creature.health = 50;
            creature.max_health = 99;
        })
        .eventai_row(row(
            509_01011,
            509_01011,
            0,
            EVENT_CREATURE_HP,
            ACTION_CAST,
            REPEAT_ONCE,
            [50, 50, 0, 0, 0, 0],
            [111, 0, 0],
        ));

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 111, TARGET)]);
}

#[test]
fn range_endpoints_are_inclusive() {
    let mut scenario = world().eventai_row(row(
        509_0102,
        509_0102,
        0,
        EVENT_TARGET_RANGE,
        ACTION_CAST,
        REPEAT_ONCE,
        [3, 3, 0, 0, 0, 0],
        [102, 0, 0],
    ));

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 102, TARGET)]);
}

#[test]
fn missing_imported_text_stays_silent_and_multiple_texts_roll_deterministically() {
    let mut scenario = world()
        .eventai_broadcast(7, "first", 0)
        .eventai_broadcast(8, "second", 0)
        .eventai_row(row(
            509_0125,
            509_0125,
            0,
            EVENT_CREATURE_HP,
            ACTION_SAY,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [999, 0, 0],
        ))
        .eventai_row(row(
            509_0126,
            509_0126,
            0,
            EVENT_CREATURE_HP,
            ACTION_SAY,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [7, 8, 0],
        ))
        .rolls([1]);

    fire(&mut scenario);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "second".to_string())]
    );
}

#[test]
fn broadcast_text_emits_its_first_animation_emote_immediately() {
    let mut scenario = world()
        .eventai_broadcast_emote(9, "move", 0, 17)
        .eventai_row(row(
            509_0127,
            509_0127,
            0,
            EVENT_CREATURE_HP,
            ACTION_SAY,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [9, 0, 0],
        ));

    fire(&mut scenario);

    assert_eq!(scenario.eventai_emotes(), vec![(CREATURE, 0, 17, 0)]);
}

#[test]
fn wounded_friendly_is_selected_as_the_event_target() {
    let mut action = row(
        509_0103,
        509_0103,
        0,
        EVENT_FRIENDLY_HP_DEFICIT,
        ACTION_CAST,
        REPEAT_ONCE,
        [40, 8, 0, 0, 0, 0],
        [103, 0, 0],
    );
    action.target_policy = TARGET_EVENT;
    let mut scenario = world()
        .creature(FRIEND, point(4.0))
        .entry(FRIEND, ENTRY + 1)
        .hurt(FRIEND, 50)
        .attacking(FRIEND, TARGET)
        .eventai_row(action);

    fire(&mut scenario);

    assert!(scenario.eventai_diagnostics().is_empty());
    assert_eq!(scenario.casts(), vec![(CREATURE, 103, FRIEND)]);
}

#[test]
fn friendly_health_requires_a_strict_deficit_and_honors_exclude_caster() {
    let mut equal = row(
        509_01031,
        509_01031,
        0,
        EVENT_FRIENDLY_HP_DEFICIT,
        ACTION_CAST,
        REPEAT_ONCE,
        [50, 8, 0, 0, 0, 0],
        [131, 0, 0],
    );
    equal.target_policy = TARGET_EVENT;
    let mut at_threshold = world()
        .creature(FRIEND, point(4.0))
        .entry(FRIEND, ENTRY + 1)
        .hurt(FRIEND, 50)
        .attacking(FRIEND, TARGET)
        .eventai_row(equal);

    fire(&mut at_threshold);

    assert!(at_threshold.casts().is_empty());

    let mut exclude = row(
        509_01032,
        509_01032,
        0,
        EVENT_FRIENDLY_HP_DEFICIT,
        ACTION_CAST,
        REPEAT_ONCE,
        [10, 8, 0, 0, 0, 0],
        [132, 0, 0],
    );
    exclude.target_policy = TARGET_EVENT;
    let mut excluded = world()
        .hurt(CREATURE, 10)
        .creature(FRIEND, point(4.0))
        .entry(FRIEND, ENTRY + 1)
        .hurt(FRIEND, 50)
        .attacking(FRIEND, TARGET)
        .eventai_exclude_caster_spell(132)
        .eventai_row(exclude);

    fire(&mut excluded);

    assert_eq!(excluded.casts(), vec![(CREATURE, 132, FRIEND)]);
}

#[test]
fn friendly_health_percent_mode_ranks_by_percent_missing() {
    let larger_absolute_deficit = SECOND_TARGET;
    let scenario = world()
        .creature(FRIEND, point(4.0))
        .entry(FRIEND, ENTRY + 1)
        .tweak(FRIEND, |friend| {
            friend.health = 20;
            friend.max_health = 100;
        })
        .attacking(FRIEND, TARGET)
        .creature(larger_absolute_deficit, point(5.0))
        .entry(larger_absolute_deficit, ENTRY + 2)
        .tweak(larger_absolute_deficit, |friend| {
            friend.health = 500;
            friend.max_health = 1_000;
        })
        .attacking(larger_absolute_deficit, TARGET);
    let rule = condition_rule(
        509_01033,
        EventCondition::FriendlyHealthDeficit(FriendlyHealthDeficitCondition {
            missing_health: 10,
            radius_yd: 8,
            percent: true,
        }),
    );
    let context = EventContext {
        engaged: true,
        current_target_guid: Some(TARGET),
        ..EventContext::empty(EventKind::FriendlyHpDeficit, CREATURE, 0)
    };

    let selected = condition(&scenario, &context, &rule, 0);

    assert_eq!(
        selected.and_then(|context| context.event_target_guid),
        Some(FRIEND)
    );
}

#[test]
fn friendly_health_ignores_wounded_characters_and_owned_pets() {
    let pet = CREATURE - 1;
    let scenario = world()
        .tweak_player(TARGET, |player| player.faction_template = BEASTS)
        .player_health(TARGET, 10, 100)
        .pet(pet, TARGET, point(2.0))
        .hurt(pet, 10)
        .attacking(pet, TARGET);
    let rule = condition_rule(
        509_010331,
        EventCondition::FriendlyHealthDeficit(FriendlyHealthDeficitCondition {
            missing_health: 10,
            radius_yd: 8,
            percent: false,
        }),
    );
    let context = EventContext {
        engaged: true,
        current_target_guid: Some(TARGET),
        ..EventContext::empty(EventKind::FriendlyHpDeficit, CREATURE, 0)
    };

    assert!(condition(&scenario, &context, &rule, 0).is_none());
}

#[test]
fn friendly_crowd_control_selects_an_engaged_creature_not_a_character() {
    let pet = CREATURE - 1;
    let scenario = world()
        .tweak_player(TARGET, |player| player.faction_template = BEASTS)
        .pet(pet, TARGET, point(2.0))
        .attacking(pet, TARGET)
        .crowd_controlled(pet)
        .creature(FRIEND, point(4.0))
        .entry(FRIEND, ENTRY + 1)
        .attacking(FRIEND, TARGET)
        .crowd_controlled(FRIEND);
    scenario.frozen.borrow_mut().insert(TARGET);
    let rule = condition_rule(
        509_01034,
        EventCondition::FriendlyCrowdControlled(FriendlyCrowdControlCondition { radius_yd: 8 }),
    );
    let context = EventContext {
        engaged: true,
        current_target_guid: Some(TARGET),
        ..EventContext::empty(EventKind::FriendlyCrowdControlled, CREATURE, 0)
    };

    let selected = condition(&scenario, &context, &rule, 0);

    assert_eq!(
        selected.and_then(|context| context.event_target_guid),
        Some(FRIEND)
    );
}

#[test]
fn friendly_crowd_control_accepts_a_slowed_creature() {
    let scenario = world()
        .creature(FRIEND, point(4.0))
        .entry(FRIEND, ENTRY + 1)
        .attacking(FRIEND, TARGET)
        .slowed(FRIEND);
    let rule = condition_rule(
        509_010341,
        EventCondition::FriendlyCrowdControlled(FriendlyCrowdControlCondition { radius_yd: 8 }),
    );
    let context = EventContext {
        engaged: true,
        current_target_guid: Some(TARGET),
        ..EventContext::empty(EventKind::FriendlyCrowdControlled, CREATURE, 0)
    };

    assert_eq!(
        condition(&scenario, &context, &rule, 0).and_then(|context| context.event_target_guid),
        Some(FRIEND)
    );
}

#[test]
fn friendly_missing_aura_modes_match_actor_and_candidate_combat_state() {
    let spell_id = 700;
    let pet = CREATURE - 1;
    let engaged_actor = world()
        .carrying(CREATURE, spell_id)
        .pet(pet, TARGET, point(2.0))
        .attacking(pet, TARGET)
        .creature(FRIEND, point(4.0))
        .entry(FRIEND, ENTRY + 1);
    let engaged_context = EventContext {
        engaged: true,
        current_target_guid: Some(TARGET),
        ..EventContext::empty(EventKind::FriendlyMissingAura, CREATURE, 0)
    };
    let nearby = condition_rule(
        509_01035,
        EventCondition::FriendlyMissingAura(FriendlyMissingAuraCondition {
            spell_id,
            radius_yd: 8,
            selection: FriendlyAuraSelection::NearbyWhileEngaged,
        }),
    );
    assert!(condition(&engaged_actor, &engaged_context, &nearby, 0).is_none());
    engaged_actor.fights.borrow_mut().push(Engagement {
        attacker: FRIEND,
        victim: TARGET,
        instance_id: INSTANCE,
        player_never_swung: false,
    });
    assert_eq!(
        condition(&engaged_actor, &engaged_context, &nearby, 0)
            .and_then(|context| context.event_target_guid),
        Some(FRIEND)
    );

    let disengaged_actor = Scenario::new(0)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .carrying(CREATURE, spell_id)
        .creature(FRIEND, point(4.0))
        .entry(FRIEND, ENTRY + 1);
    let disengaged_context = EventContext::empty(EventKind::FriendlyMissingAura, CREATURE, 0);
    let follows_actor = condition_rule(
        509_01036,
        EventCondition::FriendlyMissingAura(FriendlyMissingAuraCondition {
            spell_id,
            radius_yd: 8,
            selection: FriendlyAuraSelection::MatchActorCombatState,
        }),
    );
    assert_eq!(
        condition(&disengaged_actor, &disengaged_context, &follows_actor, 0)
            .and_then(|context| context.event_target_guid),
        Some(FRIEND)
    );

    let while_disengaged = condition_rule(
        509_01037,
        EventCondition::FriendlyMissingAura(FriendlyMissingAuraCondition {
            spell_id,
            radius_yd: 8,
            selection: FriendlyAuraSelection::AnyWhileDisengaged,
        }),
    );
    assert!(condition(&engaged_actor, &engaged_context, &while_disengaged, 0).is_none());
    assert_eq!(
        condition(&disengaged_actor, &disengaged_context, &while_disengaged, 0,)
            .and_then(|context| context.event_target_guid),
        Some(FRIEND)
    );
}

#[test]
fn ordered_text_emote_cast_and_phase_actions_share_one_rule() {
    let mut emote = row(
        509_0105,
        509_0104,
        1,
        EVENT_CREATURE_HP,
        ACTION_EMOTE,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [7, 8, 0],
    );
    emote.target_policy = TARGET_CURRENT;
    let mut scenario = world()
        .eventai_broadcast(4, "first", 0)
        .eventai_row(row(
            509_0104,
            509_0104,
            0,
            EVENT_CREATURE_HP,
            ACTION_SAY,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [4, 0, 0],
        ))
        .eventai_row(emote)
        .eventai_row(row(
            509_0106,
            509_0104,
            2,
            EVENT_CREATURE_HP,
            ACTION_CAST,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [106, 0, 0],
        ))
        .eventai_row(row(
            509_0107,
            509_0104,
            3,
            EVENT_CREATURE_HP,
            ACTION_SET_PHASE,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [1, 0, 0],
        ));

    fire(&mut scenario);

    assert_eq!(scenario.eventai_speech().len(), 1);
    assert_eq!(scenario.eventai_emotes(), vec![(CREATURE, 0, 7, TARGET)]);
    assert_eq!(scenario.casts(), vec![(CREATURE, 106, TARGET)]);
    assert_eq!(scenario.eventai_creature_state(CREATURE).phase, 1);
}

#[test]
fn a_combat_action_cast_refusal_leaves_its_rule_ready_to_retry() {
    let mut cast = row(
        509_0108,
        509_0108,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [108, 0, 0],
    );
    cast.source_flags = SOURCE_FLAG_COMBAT_ACTION;
    let mut scenario = world().eventai_row(cast).not_ready(108);

    fire(&mut scenario);
    assert!(scenario
        .eventai_state(CREATURE, 509_0108)
        .is_some_and(|state| state.invocation_started && !state.consumed));

    scenario.not_ready.borrow_mut().remove(&108);
    fire(&mut scenario);
    assert_eq!(scenario.casts(), vec![(CREATURE, 108, TARGET)]);
}

#[test]
fn random_action_runs_one_stably_selected_action() {
    let mut first = row(
        509_0110,
        509_0109,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [110, 0, 0],
    );
    first.source_flags = SOURCE_FLAG_RANDOM_ACTION;
    let mut second = row(
        509_0111,
        509_0109,
        1,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [111, 0, 0],
    );
    second.source_flags = SOURCE_FLAG_RANDOM_ACTION;
    let mut scenario = world().eventai_row(first).eventai_row(second).rolls([1]);

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 111, TARGET)]);
}

#[test]
fn a_random_combat_action_keeps_its_selected_branch_across_a_retry() {
    let mut first = row(
        509_01101,
        509_01100,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [110, 0, 0],
    );
    first.source_flags = SOURCE_FLAG_RANDOM_ACTION | SOURCE_FLAG_COMBAT_ACTION;
    let mut second = row(
        509_01102,
        509_01100,
        1,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [111, 0, 0],
    );
    second.source_flags = SOURCE_FLAG_RANDOM_ACTION | SOURCE_FLAG_COMBAT_ACTION;
    let mut scenario = world()
        .eventai_row(first)
        .eventai_row(second)
        .not_ready(111)
        .rolls([1]);

    fire(&mut scenario);
    scenario.not_ready.borrow_mut().remove(&111);
    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 111, TARGET)]);
}

#[test]
fn target_policies_choose_the_expected_actor() {
    let policies = [
        (TARGET_CURRENT, TARGET),
        (TARGET_SELF, CREATURE),
        (TARGET_TOP_THREAT, TARGET),
        (TARGET_SECOND_THREAT, SECOND_TARGET),
        (TARGET_RANDOM_THREAT, SECOND_TARGET),
        (TARGET_INVOKER, TARGET),
        (TARGET_EVENT, TARGET),
        (TARGET_TOP_THREAT_PLAYER, TARGET),
        (TARGET_RANDOM_THREAT_PLAYER, SECOND_TARGET),
        (TARGET_NEAREST_AREA, TARGET),
        (TARGET_FARTHEST_HOSTILE, SECOND_TARGET),
    ];
    let mut scenario = world()
        .player(SECOND_TARGET, point(6.0))
        .at_war(BEASTS, ALLIANCE)
        .threat(CREATURE, TARGET, 100)
        .threat(CREATURE, SECOND_TARGET, 90)
        .rolls([1, 1]);
    for (order, (policy, _)) in policies.iter().enumerate() {
        let mut action = row(
            509_0200 + order as u64,
            509_0200,
            order as u8,
            EVENT_CREATURE_HP,
            ACTION_CAST,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [200 + order as u32, 0, 0],
        );
        action.target_policy = *policy;
        scenario = scenario.eventai_row(action);
    }

    fire(&mut scenario);

    assert_eq!(
        scenario.casts(),
        vec![
            (CREATURE, 200, TARGET),
            (CREATURE, 201, CREATURE),
            (CREATURE, 202, TARGET),
            (CREATURE, 203, SECOND_TARGET),
            (CREATURE, 204, SECOND_TARGET),
            (CREATURE, 207, TARGET),
            (CREATURE, 208, SECOND_TARGET),
            (CREATURE, 209, 0),
            (CREATURE, 210, SECOND_TARGET),
        ],
        "cycle conditions provide neither invoker nor event subject, and caster-area keeps no direct target"
    );
}

#[test]
fn ranked_threat_orders_by_threat_and_breaks_a_tie_on_the_lower_guid() {
    let third_target = SECOND_TARGET + 1;
    let mut top = row(
        509_0230,
        509_0230,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [230, 0, 0],
    );
    top.target_policy = TARGET_TOP_THREAT;
    let mut second = row(
        509_0231,
        509_0230,
        1,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [231, 0, 0],
    );
    second.target_policy = TARGET_SECOND_THREAT;
    let mut scenario = world()
        .player(SECOND_TARGET, point(6.0))
        .player(third_target, point(9.0))
        .threat(CREATURE, TARGET, 50)
        .threat(CREATURE, SECOND_TARGET, 90)
        .threat(CREATURE, third_target, 90)
        .eventai_row(top)
        .eventai_row(second);

    fire(&mut scenario);

    assert_eq!(
        scenario.casts(),
        vec![
            (CREATURE, 230, SECOND_TARGET),
            (CREATURE, 231, third_target)
        ],
        "the list runs highest threat first, and a tied pair keeps the lower guid ahead"
    );
}

#[test]
fn a_taunt_keeps_current_opponent_distinct_from_highest_threat() {
    let mut current = row(
        509_02301,
        509_0230,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [234, 0, 0],
    );
    current.target_policy = TARGET_CURRENT;
    let mut highest = row(
        509_02302,
        509_0230,
        1,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [235, 0, 0],
    );
    highest.target_policy = TARGET_TOP_THREAT;
    let mut scenario = world()
        .player(SECOND_TARGET, point(6.0))
        .threat(CREATURE, TARGET, 10)
        .threat(CREATURE, SECOND_TARGET, 100)
        .taunted(CREATURE, TARGET)
        .eventai_row(current)
        .eventai_row(highest);

    fire(&mut scenario);

    assert_eq!(
        scenario.casts(),
        vec![(CREATURE, 234, TARGET), (CREATURE, 235, SECOND_TARGET)]
    );
}

#[test]
fn nearest_area_picks_the_closest_threat_holder_not_the_top_threat() {
    let mut action = row(
        509_0232,
        509_0232,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [232, 0, 0],
    );
    action.target_policy = TARGET_NEAREST_AREA;
    let mut scenario = world()
        .player(SECOND_TARGET, point(6.0))
        .threat(CREATURE, TARGET, 10)
        .threat(CREATURE, SECOND_TARGET, 200)
        .eventai_row(action);

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 232, 0)]);
    assert_eq!(
        scenario.eventai_spell_starts.borrow().as_slice(),
        &[((
            SpellStartMode::Direct,
            SpellCastTarget::CasterArea,
            false,
            SpellCasterAdmission::Living,
        ))]
    );
}

#[test]
fn no_explicit_target_checks_the_caster_without_inventing_a_unit_target() {
    let mut action = row(
        509_02321,
        509_02321,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [521, 0, 0],
    );
    action.target_policy = TARGET_NO_EXPLICIT;
    action.cast_options = CAST_AURA_ABSENT;
    let mut blocked = world().carrying(CREATURE, 521).eventai_row(action.clone());
    fire(&mut blocked);
    assert!(blocked.casts().is_empty());

    let mut ready = world().eventai_row(action);
    fire(&mut ready);

    assert_eq!(ready.casts(), vec![(CREATURE, 521, 0)]);
    assert_eq!(
        ready.eventai_spell_starts.borrow().as_slice(),
        &[((
            SpellStartMode::Direct,
            SpellCastTarget::None,
            false,
            SpellCasterAdmission::Living,
        ))]
    );
}

#[test]
fn caster_area_requires_an_eligible_candidate_but_keeps_the_area_target() {
    let creature_candidate = CREATURE + 40;
    let mut action = row(
        509_02322,
        509_02322,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [522, 0, 0],
    );
    action.target_policy = TARGET_NEAREST_AREA;
    action.cast_options = CAST_PLAYER_ONLY;
    let mut blocked = Scenario::new(0)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .creature(creature_candidate, point(3.0))
        .entry(creature_candidate, ENTRY + 1)
        .at_war(BEASTS, BEASTS + 1)
        .faction(creature_candidate, BEASTS + 1)
        .attacking(CREATURE, creature_candidate)
        .threat(CREATURE, creature_candidate, 100)
        .eventai_row(action.clone());
    fire(&mut blocked);
    assert!(blocked.casts().is_empty());

    let mut ready = world().threat(CREATURE, TARGET, 100).eventai_row(action);
    fire(&mut ready);

    assert_eq!(ready.casts(), vec![(CREATURE, 522, 0)]);
    assert_eq!(
        ready.eventai_spell_starts.borrow().as_slice(),
        &[((
            SpellStartMode::Direct,
            SpellCastTarget::CasterArea,
            false,
            SpellCasterAdmission::Living,
        ))]
    );
}

#[test]
fn triggered_force_target_self_does_not_reroll_after_the_selected_caster_refuses() {
    let spell_id = 523;
    let mut rule = condition_rule(
        509_02323,
        EventCondition::CreatureHealth(CreatureHealthCondition {
            min_pct: 0,
            max_pct: 100,
            allow_out_of_combat: false,
        }),
    );
    rule.instructions = vec![CreatureInstruction::Cast(CastInstruction {
        spell_id,
        target: InstructionTarget::RandomThreat,
        interrupt_previous: false,
        start_mode: SpellStartMode::Triggered,
        caster_role: SpellCasterRole::Selected,
        target_role: SpellTargetRole::Caster,
        aura_absent: true,
        character_only: false,
        target_must_be_casting: false,
        main_spell: false,
        distance_after_start: false,
    })];
    let subject = EventAiSubject::Entry(ENTRY);
    let definition = EventAiDefinition {
        subject,
        revision: normalized_revision(subject, std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = world()
        .player(SECOND_TARGET, point(30.0))
        .threat(CREATURE, TARGET, 10)
        .threat(CREATURE, SECOND_TARGET, 100)
        .carrying(SECOND_TARGET, spell_id)
        .rolls([0])
        .eventai_native_definition(definition);

    fire(&mut scenario);

    assert!(scenario.casts().is_empty());
}

#[test]
fn switched_cast_filters_both_the_selected_unit_and_the_final_actor_target() {
    let spell_id = 524;
    let mut rule = condition_rule(
        509_02324,
        EventCondition::CreatureHealth(CreatureHealthCondition {
            min_pct: 0,
            max_pct: 100,
            allow_out_of_combat: false,
        }),
    );
    rule.instructions = vec![CreatureInstruction::Cast(CastInstruction {
        spell_id,
        target: InstructionTarget::HighestThreat,
        interrupt_previous: false,
        start_mode: SpellStartMode::Direct,
        caster_role: SpellCasterRole::Selected,
        target_role: SpellTargetRole::Actor,
        aura_absent: true,
        character_only: false,
        target_must_be_casting: false,
        main_spell: false,
        distance_after_start: false,
    })];
    let subject = EventAiSubject::Entry(ENTRY);
    let definition = EventAiDefinition {
        subject,
        revision: normalized_revision(subject, std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let blocked_definition = definition.clone();
    let mut blocked = world()
        .player(SECOND_TARGET, point(3.0))
        .threat(CREATURE, TARGET, 10)
        .threat(CREATURE, SECOND_TARGET, 100)
        .carrying(CREATURE, spell_id)
        .eventai_native_definition(blocked_definition);

    fire(&mut blocked);
    assert!(blocked.casts().is_empty());

    let mut scenario = world()
        .player(SECOND_TARGET, point(3.0))
        .threat(CREATURE, TARGET, 10)
        .threat(CREATURE, SECOND_TARGET, 100)
        .eventai_native_definition(definition);

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(SECOND_TARGET, spell_id, CREATURE)]);
}

#[test]
fn farthest_hostile_skips_a_target_inside_melee_reach() {
    let mut action = row(
        509_0233,
        509_0233,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [233, 0, 0],
    );
    action.target_policy = TARGET_FARTHEST_HOSTILE;
    let mut scenario = world().threat(CREATURE, TARGET, 100).eventai_row(action);

    fire(&mut scenario);

    assert!(
        scenario.casts().is_empty(),
        "a knockback-shaped cast aimed at the farthest hostile must never land on the victim \
         already standing in melee reach"
    );
}

#[test]
fn call_for_help_recruits_only_inside_its_authored_radius() {
    let near_friend = CREATURE + 30;
    let far_friend = CREATURE + 31;
    let mut scenario = world()
        .creature(near_friend, point(6.0))
        .entry(near_friend, ENTRY + 30)
        .creature(far_friend, point(20.0))
        .entry(far_friend, ENTRY + 31)
        .eventai_row(row(
            509_0234,
            509_0234,
            0,
            EVENT_CREATURE_HP,
            ACTION_CALL_FOR_HELP,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [8, 0, 0],
        ));

    fire(&mut scenario);

    let pulls = scenario.pulls.borrow().clone();
    assert!(pulls.contains(&(near_friend, TARGET, Pull::Assisted)));
    assert!(
        !pulls.iter().any(|(helper, _, _)| *helper == far_friend),
        "a friend beyond the authored radius hears nothing"
    );
}

#[test]
fn ranked_random_casts_filter_ineligible_targets_before_selection() {
    let mut action = row(
        509_0220,
        509_0220,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [220, 0, 0],
    );
    action.target_policy = TARGET_RANDOM_THREAT;
    action.cast_options = CAST_AURA_ABSENT;
    let mut scenario = world()
        .player(SECOND_TARGET, point(6.0))
        .threat(CREATURE, TARGET, 100)
        .threat(CREATURE, SECOND_TARGET, 90)
        .spell_range(220, 10)
        .eventai_row(action)
        .rolls([0]);
    scenario.auras.borrow_mut().insert((TARGET, 220));

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 220, SECOND_TARGET)]);
}

#[test]
fn source_rule_order_wins_across_engaged_event_kinds() {
    let phase = row(
        509_0300,
        300,
        0,
        EVENT_FRIENDLY_HP_DEFICIT,
        ACTION_SET_PHASE,
        REPEAT_ONCE,
        [1, 8, 0, 0, 0, 0],
        [1, 0, 0],
    );
    let say = row(
        509_0301,
        301,
        0,
        EVENT_CREATURE_HP,
        ACTION_SAY,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [10, 0, 0],
    );
    let mut scenario = world()
        .hurt(CREATURE, 1)
        .eventai_broadcast(10, "too late", 0)
        .eventai_row(phase)
        .eventai_row(say);

    fire(&mut scenario);

    assert!(scenario.eventai_speech().is_empty());
    assert_eq!(scenario.eventai_creature_state(CREATURE).phase, 1);
}

#[test]
fn triggered_cast_starts_during_a_pending_direct_cast() {
    let mut triggered = row(
        509_0302,
        302,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [300, 0, 0],
    );
    triggered.cast_options = CAST_TRIGGERED;
    let pending = row(
        509_0303,
        303,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [301, 0, 0],
    );
    let mut scenario = world().eventai_row(triggered).eventai_row(pending);
    scenario.casting.borrow_mut().insert(CREATURE);

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 300, TARGET)]);
    assert_eq!(
        scenario.eventai_spell_starts.borrow().as_slice(),
        &[((
            SpellStartMode::Triggered,
            SpellCastTarget::Unit(TARGET),
            false,
            SpellCasterAdmission::Living,
        ))]
    );
}

#[test]
fn direct_cast_refuses_without_interrupt_and_preserves_an_active_channel() {
    let direct = row(
        509_03031,
        509_03031,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [306, 0, 0],
    );
    let mut scenario = world().channeling(CREATURE).eventai_row(direct);

    fire(&mut scenario);

    assert!(scenario.casts().is_empty());
    assert!(scenario.channeling.borrow().contains(&CREATURE));
}

#[test]
fn triggered_cast_starts_during_an_active_channel() {
    let mut triggered = row(
        509_03032,
        509_03032,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [307, 0, 0],
    );
    triggered.cast_options = CAST_TRIGGERED;
    let mut scenario = world().channeling(CREATURE).eventai_row(triggered);

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 307, TARGET)]);
    assert!(scenario.channeling.borrow().contains(&CREATURE));
}

#[test]
fn interrupting_triggered_cast_breaks_an_active_channel() {
    let mut triggered = row(
        509_030321,
        509_030321,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [309, 0, 0],
    );
    triggered.cast_options = CAST_TRIGGERED | CAST_INTERRUPT_PREVIOUS;
    let mut scenario = world().channeling(CREATURE).eventai_row(triggered);

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 309, TARGET)]);
    assert!(!scenario.channeling.borrow().contains(&CREATURE));
}

#[test]
fn target_casting_conditions_and_cast_filters_recognize_an_active_channel() {
    let mut target_casting_rule = condition_rule(509_03033, EventCondition::TargetCasting);
    target_casting_rule.instructions = vec![CreatureInstruction::Emote(EmoteInstruction {
        emote_id: 7,
        target: InstructionTarget::CurrentOpponent,
    })];
    let mut cast_rule = condition_rule(
        509_03034,
        EventCondition::CreatureHealth(CreatureHealthCondition {
            min_pct: 0,
            max_pct: 100,
            allow_out_of_combat: false,
        }),
    );
    cast_rule.instructions = vec![CreatureInstruction::Cast(CastInstruction {
        spell_id: 308,
        target: InstructionTarget::CurrentOpponent,
        interrupt_previous: false,
        start_mode: SpellStartMode::Direct,
        caster_role: SpellCasterRole::Actor,
        target_role: SpellTargetRole::Selected,
        aura_absent: false,
        character_only: false,
        target_must_be_casting: true,
        main_spell: false,
        distance_after_start: false,
    })];
    let subject = EventAiSubject::Entry(ENTRY);
    let rules = vec![target_casting_rule, cast_rule];
    let definition = EventAiDefinition {
        subject,
        revision: normalized_revision(subject, &rules),
        rules,
    };
    let mut scenario = world()
        .channeling(TARGET)
        .eventai_native_definition(definition);

    fire(&mut scenario);

    assert_eq!(scenario.eventai_emotes(), vec![(CREATURE, 0, 7, TARGET)]);
    assert_eq!(scenario.casts(), vec![(CREATURE, 308, TARGET)]);
}

#[test]
fn a_combat_action_waits_while_the_actor_channels() {
    let mut rule = condition_rule(
        509_03035,
        EventCondition::CreatureHealth(CreatureHealthCondition {
            min_pct: 0,
            max_pct: 100,
            allow_out_of_combat: false,
        }),
    );
    rule.execution = ExecutionPolicy::CombatAction;
    rule.instructions = vec![CreatureInstruction::Emote(EmoteInstruction {
        emote_id: 8,
        target: InstructionTarget::SelfActor,
    })];
    let subject = EventAiSubject::Entry(ENTRY);
    let definition = EventAiDefinition {
        subject,
        revision: normalized_revision(subject, std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = world()
        .channeling(CREATURE)
        .eventai_native_definition(definition);

    fire(&mut scenario);

    assert!(scenario.eventai_emotes().is_empty());
    assert!(scenario.eventai_state(CREATURE, 509_03035).is_none());
}

#[test]
fn interrupt_previous_replaces_a_pending_cast() {
    let mut action = row(
        509_0304,
        304,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [304, 0, 0],
    );
    action.cast_options = CAST_INTERRUPT_PREVIOUS;
    let mut scenario = world().eventai_row(action);
    scenario.casting.borrow_mut().insert(CREATURE);

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 304, TARGET)]);
    assert!(!scenario.casting.borrow().contains(&CREATURE));
}

#[test]
fn a_refused_interrupting_start_preserves_the_pending_cast() {
    let mut action = row(
        509_03041,
        509_03041,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [305, 0, 0],
    );
    action.cast_options = CAST_INTERRUPT_PREVIOUS;
    let mut scenario = world()
        .eventai_row(action)
        .mid_cast(CREATURE)
        .not_ready(305);

    fire(&mut scenario);

    assert!(scenario.casts().is_empty());
    assert!(scenario.casting.borrow().contains(&CREATURE));
}

#[test]
fn an_authored_flee_runs_a_creature_the_fixed_rout_would_leave_standing() {
    // A beast at 30% health: below no flee threshold and of a kind that fights to the death, so
    // nothing but the authored rule can break it off.
    let mut scenario = world().hurt(CREATURE, 30).eventai_row(row(
        509_0211,
        509_0211,
        0,
        EVENT_CREATURE_HP,
        ACTION_FLEE_FOR_ASSIST,
        REPEAT_ONCE,
        [0, 30, 0, 0, 0, 0],
        [0; 3],
    ));

    fire(&mut scenario);
    assert!(
        scenario.rout_ends_ms(CREATURE) > 0,
        "the authored action is the only thing that can open this creature's rout window"
    );

    // The rout pass runs BEFORE the eventai pass in the firing order, so the window opened above is
    // read on the next firing.
    fire(&mut scenario);

    let legs = scenario.effects();
    assert_eq!(
        legs.len(),
        1,
        "an authored flee that stamps a window nothing then reads leaves the creature standing in \
         melee, which is the fixed rout's own gate deciding a fight the script owns"
    );
    assert!(
        legs[0].dest.x < 0.0,
        "the leg must run AWAY from the victim it is breaking off from"
    );
}

#[test]
fn a_repeat_authored_flee_runs_the_creature_again_later_in_the_fight() {
    // The rule repeats every 15 s and each rout window runs 10 s, so the second firing lands on a
    // window that is stamped but SPENT. It must re-stamp rather than treat the spent window as the
    // fixed rout's once per Engagement.
    let mut scenario = world().hurt(CREATURE, 30).eventai_row(row(
        509_0214,
        509_0214,
        0,
        EVENT_CREATURE_HP,
        ACTION_FLEE_FOR_ASSIST,
        REPEAT,
        [0, 30, 15_000, 15_000, 0, 0],
        [0; 3],
    ));

    fire(&mut scenario); // the rule stamps the first window
    fire(&mut scenario); // the rout pass reads it: the first flee leg

    scenario.advance_clock(15_000_000); // the first window and the repeat wait are both over
    fire(&mut scenario); // the rule fires onto the spent window and re-stamps it
    scenario.advance_clock(5_000_000); // the chase leg thrown while the window was spent lands
    fire(&mut scenario); // the re-stamped window runs the creature again

    let flee_legs: Vec<_> = scenario
        .effects()
        .into_iter()
        .filter(|leg| leg.dur_ms > 0 && leg.dest.x < leg.start.x)
        .collect();
    assert!(
        flee_legs.len() >= 2,
        "a later flee in the same fight must write another leg away from the victim"
    );
}

#[test]
fn call_for_help_uses_the_normal_assisted_engagement() {
    let helper = CREATURE + 20;
    let mut scenario = world()
        .creature(helper, point(4.0))
        .entry(helper, ENTRY + 20)
        .eventai_row(row(
            509_0212,
            509_0212,
            0,
            EVENT_CREATURE_HP,
            ACTION_CALL_FOR_HELP,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [8, 0, 0],
        ));

    fire(&mut scenario);

    assert!(scenario
        .pulls
        .borrow()
        .contains(&(helper, TARGET, Pull::Assisted)));
}

#[test]
fn an_engaged_cast_rule_suppresses_only_its_subject_even_when_the_condition_is_false() {
    let scripted = CREATURE + 10;
    let mut scenario = world()
        .creature(scripted, point(1.0))
        .entry(scripted, ENTRY + 2)
        .attacking(scripted, TARGET)
        .lone_spell(CREATURE, 120)
        .lone_spell(scripted, 121)
        .eventai_row(row(
            509_0112,
            509_0112,
            0,
            EVENT_CREATURE_HP,
            ACTION_CAST,
            REPEAT_ONCE,
            [0, 0, 0, 0, 0, 0],
            [112, 0, 0],
        ));

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(scripted, 121, TARGET)]);
}

#[test]
fn out_of_combat_cast_rules_do_not_suppress_flat_cast() {
    let mut rule = condition_rule(
        509_01121,
        EventCondition::TimedOutOfCombat(TimeWindow {
            min_ms: 0,
            max_ms: 0,
        }),
    );
    rule.instructions = vec![CreatureInstruction::Cast(CastInstruction {
        spell_id: 112,
        target: InstructionTarget::CurrentOpponent,
        interrupt_previous: false,
        start_mode: SpellStartMode::Direct,
        caster_role: SpellCasterRole::Actor,
        target_role: SpellTargetRole::Selected,
        aura_absent: false,
        character_only: false,
        target_must_be_casting: false,
        main_spell: false,
        distance_after_start: false,
    })];
    let subject = EventAiSubject::Entry(ENTRY);
    let definition = EventAiDefinition {
        subject,
        revision: normalized_revision(subject, std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = world()
        .lone_spell(CREATURE, 120)
        .eventai_native_definition(definition);

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 120, TARGET)]);
}

#[test]
fn out_of_combat_sight_filters_conditions_before_choosing_the_nearest_invoker() {
    let nearer = TARGET;
    let farther = SECOND_TARGET;
    let quest_entry = 772;
    let mut rule = condition_rule(
        509_01122,
        EventCondition::OutOfCombatSight(OutOfCombatSightCondition {
            require_non_hostile: false,
            max_range_yd: 20,
            character_only: true,
            predicate: EventPredicate::QuestTaken(QuestTakenPredicate { quest_entry }),
        }),
    );
    rule.instructions = vec![CreatureInstruction::Emote(EmoteInstruction {
        emote_id: 7,
        target: InstructionTarget::Invoker,
    })];
    let subject = EventAiSubject::Entry(ENTRY);
    let definition = EventAiDefinition {
        subject,
        revision: normalized_revision(subject, std::slice::from_ref(&rule)),
        rules: vec![rule],
    };
    let mut scenario = Scenario::new(0)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(nearer, point(2.0))
        .player(farther, point(4.0))
        .at_war(BEASTS, ALLIANCE)
        .eventai_quest_taken(farther, quest_entry)
        .eventai_native_definition(definition);

    let active = HashSet::from([CREATURE]);
    let scope = scope();
    evaluate(
        &mut scenario,
        EventAiRequest::Cycle {
            scope: &scope,
            active: &active,
        },
    );

    assert_eq!(scenario.eventai_emotes(), vec![(CREATURE, 0, 7, farther)]);
}

#[test]
fn edge_cast_rows_do_not_suppress_the_default_rotation() {
    let edge = row(
        509_0310,
        310,
        0,
        EVENT_ON_DEATH,
        ACTION_CAST,
        REPEAT_ONCE,
        [0; 6],
        [310, 0, 0],
    );
    let mut scenario = world().lone_spell(CREATURE, 120).eventai_row(edge);

    fire(&mut scenario);

    assert_eq!(
        scenario.casts(),
        vec![(CREATURE, 120, TARGET)],
        "an on-death or on-spawn cast fires at a moment the rotation never covers, so it takes \
         nothing over: silencing the rotation for it would leave the creature swinging in silence"
    );
}

#[test]
fn an_accepted_eventai_cast_replaces_template_casting_for_its_lifecycle() {
    let mut scenario = Scenario::new(0)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(20.0))
        .at_war(BEASTS, ALLIANCE)
        .attacking(CREATURE, TARGET)
        .caster(CREATURE, 30.0)
        .rotation_line(CREATURE, 120, CastWhen::Always, 1)
        .eventai_row(row(
            509_0113,
            509_0113,
            0,
            EVENT_CREATURE_HP,
            ACTION_CAST,
            REPEAT_ONCE,
            [100, 100, 0, 0, 0, 0],
            [113, 0, 0],
        ));

    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 113, TARGET)]);
    assert!(
        scenario.has_leg(CREATURE),
        "an accepted EventAI cast removes the template hold range for this lifecycle"
    );

    scenario.casts.borrow_mut().clear();
    fire(&mut scenario);

    assert!(scenario.casts().is_empty());

    scenario.eventai_rule_state.borrow_mut().remove(&CREATURE);
    scenario
        .eventai_creature_state
        .borrow_mut()
        .remove(&CREATURE);
    scenario.eventai_rows.borrow_mut().clear();
    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 120, TARGET)]);
}

#[test]
fn an_accepted_eventai_cast_keeps_the_creature_spell_list_independent() {
    let mut scenario = world()
        .rotation_line(CREATURE, 120, CastWhen::Always, 20)
        .creature_spell_list_line(CREATURE, 121, CastWhen::Always, 10)
        .eventai_row(row(
            509_01131,
            509_01131,
            0,
            EVENT_CREATURE_HP,
            ACTION_CAST,
            REPEAT_ONCE,
            [0, 100, 0, 0, 0, 0],
            [113, 0, 0],
        ));

    fire(&mut scenario);

    assert_eq!(
        scenario.casts(),
        vec![(CREATURE, 113, TARGET), (CREATURE, 121, TARGET)]
    );
}

#[test]
fn guid_cast_acceptance_does_not_suppress_an_entry_peer() {
    let peer = CREATURE + 30;
    let mut cast = row(
        509_0114,
        509_0114,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [100, 100, 0, 0, 0, 0],
        [114, 0, 0],
    );
    cast.creature_entry = 0;
    cast.creature_guid = CREATURE;
    let mut scenario = world()
        .creature(peer, point(1.0))
        .entry(peer, ENTRY)
        .attacking(peer, TARGET)
        .lone_spell(CREATURE, 120)
        .rotation_line(peer, 122, CastWhen::Always, 1)
        .eventai_row(cast);

    fire(&mut scenario);

    assert_eq!(
        scenario.casts(),
        vec![(CREATURE, 114, TARGET), (peer, 122, TARGET)]
    );
}

#[test]
fn an_unscripted_creature_keeps_the_same_cycle_state_and_effects() {
    let mut unrelated = row(
        509_0320,
        320,
        0,
        EVENT_CREATURE_HP,
        ACTION_CAST,
        REPEAT_ONCE,
        [0, 100, 0, 0, 0, 0],
        [320, 0, 0],
    );
    unrelated.creature_entry = ENTRY + 99;
    let mut baseline = world().lone_spell(CREATURE, 120);
    let mut with_catalogue = world().lone_spell(CREATURE, 120).eventai_row(unrelated);

    let baseline_tick = baseline.tick(true, scope());
    let catalogue_tick = with_catalogue.tick(true, scope());
    let baseline_outcome = run_cycle(&mut baseline, baseline_tick);
    let catalogue_outcome = run_cycle(&mut with_catalogue, catalogue_tick);

    assert_eq!(baseline_outcome.awake, catalogue_outcome.awake);
    assert_eq!(
        baseline_outcome.rows_visited,
        catalogue_outcome.rows_visited
    );
    assert_eq!(baseline.snapshot(), with_catalogue.snapshot());
    assert_eq!(baseline.victims(), with_catalogue.victims());
    assert_eq!(baseline.casts(), with_catalogue.casts());
    assert_eq!(baseline.effects(), with_catalogue.effects());
}
