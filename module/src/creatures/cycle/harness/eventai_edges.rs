//! Spawn and death EventAI scenarios.
use super::*;
use crate::creatures::eventai::*;

const CREATURE: u64 = 8_101;
const TARGET: u64 = 8_102;
const ENTRY: u32 = 911;

fn point(x: f32) -> Point {
    Point { x, y: 0.0, z: 0.0 }
}

fn scenario() -> Scenario {
    Scenario::new(1_000_000)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(2.0))
}

fn row(
    row_id: u64,
    rule_id: u64,
    order: u8,
    event_type: u8,
    action_type: u8,
    chance_pct: u8,
    phases: u32,
    repeat_policy: u8,
    action_params: [u32; 3],
) -> CreatureAiEvent {
    CreatureAiEvent {
        id: row_id,
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
        chance_pct,
        allowed_phase_mask: phases,
        source_flags: 0,
        repeat_policy,
        event_param_1: 0,
        event_param_2: 0,
        event_param_3: 0,
        event_param_4: 0,
        event_param_5: 0,
        event_param_6: 0,
        action_param_1: action_params[0],
        action_param_2: action_params[1],
        action_param_3: action_params[2],
        target_policy: TARGET_CURRENT,
        cast_options: 0,
    }
}

fn edge(scenario: &mut Scenario, kind: EventKind, assisted: bool) {
    evaluate(
        scenario,
        EventAiRequest::Edge(EventContext {
            kind,
            creature_guid: CREATURE,
            invoker_guid: Some(TARGET),
            event_target_guid: Some(TARGET),
            current_target_guid: Some(TARGET),
            assisted,
            now_ms: 1_000,
        }),
    );
}

fn reset_lifecycle(scenario: &Scenario) {
    scenario.eventai_rule_state.borrow_mut().remove(&CREATURE);
    scenario
        .eventai_creature_state
        .borrow_mut()
        .remove(&CREATURE);
}

#[test]
fn direct_aggro_speaks_once_and_assisted_aggro_keeps_casts() {
    let mut direct = scenario()
        .eventai_broadcast(1, "direct", 0)
        .eventai_row(row(
            1,
            1,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT,
            [1, 0, 0],
        ));

    edge(&mut direct, EventKind::OnAggro, false);
    edge(&mut direct, EventKind::OnAggro, false);

    assert_eq!(
        direct.eventai_speech(),
        vec![(CREATURE, 0, "direct".to_string())]
    );

    let mut assisted = scenario()
        .eventai_broadcast(2, "quiet", 0)
        .eventai_row(row(
            2,
            2,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [2, 0, 0],
        ))
        .eventai_row(row(
            3,
            3,
            0,
            EVENT_ON_AGGRO,
            ACTION_CAST,
            100,
            1,
            REPEAT_ONCE,
            [3, 0, 0],
        ));

    edge(&mut assisted, EventKind::OnAggro, true);

    assert!(assisted.eventai_speech().is_empty());
    assert_eq!(assisted.casts(), vec![(CREATURE, 3, TARGET)]);
}

#[test]
fn an_edge_chance_miss_is_consumed_even_when_the_source_repeats() {
    let mut scenario = scenario()
        .eventai_broadcast(4, "late", 0)
        .eventai_row(row(
            4,
            4,
            0,
            EVENT_ON_SPAWN,
            ACTION_SAY,
            50,
            1,
            REPEAT,
            [4, 0, 0],
        ))
        .rolls([50]);

    edge(&mut scenario, EventKind::OnSpawn, false);
    edge(&mut scenario, EventKind::OnSpawn, false);

    assert!(scenario.eventai_speech().is_empty());
    assert!(scenario.eventai_state(CREATURE, 4).unwrap().consumed);
}

#[test]
fn a_repeatable_condition_keeps_its_repeat_policy() {
    let mut repeated = row(
        10,
        10,
        0,
        EVENT_CREATURE_HP,
        ACTION_SAY,
        100,
        1,
        REPEAT,
        [10, 0, 0],
    );
    repeated.event_param_2 = 100;
    let mut scenario = scenario()
        .eventai_broadcast(10, "again", 0)
        .eventai_row(repeated);

    edge(&mut scenario, EventKind::CreatureHp, false);
    edge(&mut scenario, EventKind::CreatureHp, false);

    assert_eq!(
        scenario.eventai_speech(),
        vec![
            (CREATURE, 0, "again".to_string()),
            (CREATURE, 0, "again".to_string()),
        ]
    );
    assert!(!scenario.eventai_state(CREATURE, 10).unwrap().consumed);
}

#[test]
fn spawn_and_respawn_each_get_one_new_lifecycle() {
    let mut scenario = scenario().eventai_broadcast(5, "awake", 0).eventai_row(row(
        5,
        5,
        0,
        EVENT_ON_SPAWN,
        ACTION_SAY,
        100,
        1,
        REPEAT_ONCE,
        [5, 0, 0],
    ));

    edge(&mut scenario, EventKind::OnSpawn, false);
    edge(&mut scenario, EventKind::OnSpawn, false);
    reset_lifecycle(&scenario);
    edge(&mut scenario, EventKind::OnSpawn, false);

    assert_eq!(
        scenario.eventai_speech(),
        vec![
            (CREATURE, 0, "awake".to_string()),
            (CREATURE, 0, "awake".to_string()),
        ]
    );
}

#[test]
fn a_spawn_phase_change_gates_later_rules_in_source_order() {
    let mut scenario = scenario()
        .eventai_broadcast(11, "phase two", 0)
        .eventai_row(row(
            11,
            11,
            0,
            EVENT_ON_SPAWN,
            ACTION_SET_PHASE,
            100,
            1,
            REPEAT_ONCE,
            [2, 0, 0],
        ))
        .eventai_row(row(
            12,
            12,
            0,
            EVENT_ON_SPAWN,
            ACTION_SAY,
            100,
            1 << 2,
            REPEAT_ONCE,
            [11, 0, 0],
        ));

    edge(&mut scenario, EventKind::OnSpawn, false);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "phase two".to_string())]
    );
}

#[test]
fn combat_exit_keeps_spawn_rules_consumed_in_the_same_lifecycle() {
    let mut scenario = scenario()
        .eventai_broadcast(13, "once", 0)
        .eventai_row(row(
            13,
            13,
            0,
            EVENT_ON_SPAWN,
            ACTION_SET_PHASE,
            100,
            1,
            REPEAT_ONCE,
            [1, 0, 0],
        ))
        .eventai_row(row(
            14,
            14,
            0,
            EVENT_ON_SPAWN,
            ACTION_SAY,
            100,
            1 << 1,
            REPEAT_ONCE,
            [13, 0, 0],
        ));

    edge(&mut scenario, EventKind::OnSpawn, false);
    EngageSink::leave_combat(&mut scenario, CREATURE);
    edge(&mut scenario, EventKind::OnSpawn, false);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "once".to_string())]
    );
}

#[test]
fn death_runs_before_its_lifecycle_cleanup() {
    let mut scenario = scenario()
        .eventai_broadcast(6, "gone", 0)
        .eventai_row(row(
            6,
            6,
            0,
            EVENT_ON_AGGRO,
            ACTION_SET_PHASE,
            100,
            1,
            REPEAT_ONCE,
            [1, 0, 0],
        ))
        .eventai_row(row(
            7,
            7,
            0,
            EVENT_ON_DEATH,
            ACTION_SAY,
            100,
            1 << 1,
            REPEAT_ONCE,
            [6, 0, 0],
        ));

    edge(&mut scenario, EventKind::OnAggro, false);
    edge(&mut scenario, EventKind::OnDeath, false);
    reset_lifecycle(&scenario);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "gone".to_string())]
    );
    assert!(scenario
        .eventai_creature_state
        .borrow()
        .get(&CREATURE)
        .is_none());
    assert!(scenario.eventai_state(CREATURE, 6).is_none());
    assert!(scenario.eventai_state(CREATURE, 7).is_none());
}

#[test]
fn combat_end_clears_only_engagement_state() {
    let mut scenario = scenario()
        .eventai_row(row(
            8,
            8,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [0, 0, 0],
        ))
        .eventai_row(row(
            9,
            9,
            0,
            EVENT_ON_SPAWN,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [0, 0, 0],
        ));
    scenario.eventai_creature_state.borrow_mut().insert(
        CREATURE,
        CreatureState {
            phase: 3,
            lifecycle_id: 4,
            engagement_id: 5,
            ranged_distance: 20.0,
            ranged_angle: 1.0,
        },
    );
    scenario.eventai_rule_state.borrow_mut().insert(
        CREATURE,
        HashMap::from([
            (
                8,
                RuleState {
                    next_eligible_ms: 1,
                    consumed: true,
                    lifecycle_id: 4,
                    engagement_id: 5,
                },
            ),
            (
                9,
                RuleState {
                    next_eligible_ms: 1,
                    consumed: true,
                    lifecycle_id: 4,
                    engagement_id: 5,
                },
            ),
        ]),
    );

    EngageSink::leave_combat(&mut scenario, CREATURE);

    assert!(scenario.eventai_state(CREATURE, 8).is_none());
    assert!(scenario.eventai_state(CREATURE, 9).is_some());
    assert_eq!(
        scenario.eventai_creature_state(CREATURE),
        CreatureState {
            phase: 0,
            lifecycle_id: 4,
            engagement_id: 6,
            ranged_distance: 0.0,
            ranged_angle: 0.0,
        }
    );
}
