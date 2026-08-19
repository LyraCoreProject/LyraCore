use super::*;
use crate::creatures::eventai::*;

const CREATURE: u64 = 8_001;
const TARGET: u64 = 8_002;
const ENTRY: u32 = 901;

fn point(x: f32) -> Point {
    Point { x, y: 0.0, z: 0.0 }
}

fn scope() -> TickScope {
    TickScope::CatchAll {
        dedicated: HashSet::new(),
    }
}

fn fire(scenario: &mut Scenario) -> CycleOutcome {
    let tick = scenario.tick(false, scope());
    run_cycle(scenario, tick)
}

#[expect(
    clippy::too_many_arguments,
    reason = "test rows keep every authored rule value visible at the call site"
)]
fn native_row(
    row_id: u64,
    rule_id: u64,
    order: u8,
    event: u8,
    action: u8,
    chance: u8,
    phases: u32,
    repeat: u8,
    event_params: [u32; 6],
    action_params: [u32; 3],
) -> CreatureAiEvent {
    CreatureAiEvent {
        id: row_id,
        creature_entry: ENTRY,
        event_type: event,
        action_type: action,
        text: String::new(),
        spell_id: 0,
        initial_min_ms: 0,
        initial_max_ms: 0,
        repeat_min_ms: 0,
        repeat_max_ms: 0,
        source_rule_id: rule_id,
        action_order: order,
        creature_guid: 0,
        chance_pct: chance,
        allowed_phase_mask: phases,
        source_flags: 0,
        repeat_policy: repeat,
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

fn world(now_micros: u64) -> Scenario {
    Scenario::new(now_micros)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(2.0))
}

#[test]
fn zero_source_rule_uses_the_row_id() {
    let mut legacy = native_row(
        71,
        0,
        0,
        EVENT_ON_AGGRO,
        ACTION_SAY,
        0,
        0,
        REPEAT_ONCE,
        [0; 6],
        [0; 3],
    );
    legacy.text = "legacy".to_string();
    let mut scenario = world(1_000_000).eventai_row(legacy);

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "legacy".to_string())]
    );
    assert!(scenario.eventai_state(CREATURE, 71).unwrap().consumed);
}

#[test]
fn a_guid_subject_does_not_run_for_another_creature_of_the_same_entry() {
    let other = CREATURE + 10;
    let mut row = native_row(
        81,
        80,
        0,
        EVENT_ON_AGGRO,
        ACTION_SAY,
        100,
        1,
        REPEAT_ONCE,
        [0; 6],
        [8, 0, 0],
    );
    row.creature_entry = 0;
    row.creature_guid = CREATURE;
    let mut scenario = world(1_000_000)
        .creature(other, point(1.0))
        .entry(other, ENTRY)
        .eventai_broadcast(8, "guid only", 0)
        .eventai_row(row);

    EngageSink::engage(&mut scenario, other, TARGET, Pull::Noticed);
    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    assert_eq!(
        scenario.eventai_speech(),
        vec![(CREATURE, 0, "guid only".to_string())]
    );
}

#[test]
fn one_rule_rolls_once_and_runs_actions_in_authored_order() {
    let mut scenario = world(1_000_000)
        .eventai_broadcast(11, "first", 0)
        .eventai_broadcast(12, "second", 1)
        .eventai_row(native_row(
            102,
            100,
            2,
            EVENT_ON_AGGRO,
            ACTION_YELL,
            50,
            1,
            REPEAT_ONCE,
            [0; 6],
            [12, 0, 0],
        ))
        .eventai_row(native_row(
            101,
            100,
            1,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            50,
            1,
            REPEAT_ONCE,
            [0; 6],
            [11, 0, 0],
        ))
        .rolls([49]);

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    assert_eq!(
        scenario.eventai_speech(),
        vec![
            (CREATURE, 0, "first".to_string()),
            (CREATURE, 1, "second".to_string()),
        ]
    );
}

#[test]
fn a_phase_change_gates_the_next_rule_in_the_same_dispatch() {
    let mut scenario = world(1_000_000)
        .eventai_broadcast(20, "phase one", 0)
        .eventai_row(native_row(
            201,
            200,
            0,
            EVENT_ON_AGGRO,
            ACTION_SET_PHASE,
            100,
            1,
            REPEAT_ONCE,
            [0; 6],
            [1, 0, 0],
        ))
        .eventai_row(native_row(
            211,
            210,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1 << 1,
            REPEAT_ONCE,
            [0; 6],
            [20, 0, 0],
        ));

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    assert_eq!(scenario.eventai_speech().len(), 1);
    assert_eq!(scenario.eventai_creature_state(CREATURE).phase, 1);
}

#[test]
fn a_chance_miss_consumes_the_timed_opportunity() {
    let rule_id = 300;
    let mut scenario = world(0)
        .attacking(CREATURE, TARGET)
        .eventai_broadcast(30, "tick", 0)
        .eventai_row(native_row(
            301,
            rule_id,
            0,
            EVENT_TIMED_IN_COMBAT,
            ACTION_SAY,
            50,
            1,
            REPEAT,
            [0, 0, 1_000, 1_000, 0, 0],
            [30, 0, 0],
        ))
        .rolls([50]);

    fire(&mut scenario);
    fire(&mut scenario);
    scenario.advance_clock(500_000);
    fire(&mut scenario);

    assert!(scenario.eventai_speech().is_empty());
    assert_eq!(
        scenario
            .eventai_state(CREATURE, rule_id)
            .unwrap()
            .next_eligible_ms,
        1_000
    );
}

#[test]
fn timing_windows_include_the_fixed_minimum_and_maximum() {
    let mut scenario = world(1_000_000)
        .attacking(CREATURE, TARGET)
        .eventai_row(native_row(
            401,
            400,
            0,
            EVENT_TIMED_IN_COMBAT,
            ACTION_CAST,
            100,
            1,
            REPEAT,
            [250, 250, 1_000, 1_000, 0, 0],
            [40, 0, 0],
        ))
        .eventai_row(native_row(
            411,
            410,
            0,
            EVENT_TIMED_IN_COMBAT,
            ACTION_CAST,
            100,
            1,
            REPEAT,
            [100, 101, 1_000, 1_000, 0, 0],
            [41, 0, 0],
        ))
        .eventai_row(native_row(
            421,
            420,
            0,
            EVENT_TIMED_IN_COMBAT,
            ACTION_CAST,
            100,
            1,
            REPEAT,
            [100, 101, 1_000, 1_000, 0, 0],
            [42, 0, 0],
        ))
        .rolls([0, 1]);

    fire(&mut scenario);

    assert_eq!(
        scenario
            .eventai_state(CREATURE, 400)
            .unwrap()
            .next_eligible_ms,
        1_250
    );
    assert_eq!(
        scenario
            .eventai_state(CREATURE, 410)
            .unwrap()
            .next_eligible_ms,
        1_100
    );
    assert_eq!(
        scenario
            .eventai_state(CREATURE, 420)
            .unwrap()
            .next_eligible_ms,
        1_101
    );
}

#[test]
fn direct_aggro_text_runs_once() {
    let mut scenario = world(1_000_000)
        .eventai_broadcast(50, "only once", 0)
        .eventai_row(native_row(
            501,
            500,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [0; 6],
            [50, 0, 0],
        ));

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);
    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    assert_eq!(scenario.eventai_speech().len(), 1);
}

#[test]
fn assisted_aggro_suppresses_text_but_keeps_non_text_actions() {
    let mut scenario = world(1_000_000)
        .eventai_broadcast(60, "quiet", 0)
        .eventai_row(native_row(
            601,
            600,
            0,
            EVENT_ON_AGGRO,
            ACTION_SAY,
            100,
            1,
            REPEAT_ONCE,
            [0; 6],
            [60, 0, 0],
        ))
        .eventai_row(native_row(
            602,
            600,
            1,
            EVENT_ON_AGGRO,
            ACTION_CAST,
            100,
            1,
            REPEAT_ONCE,
            [0; 6],
            [61, 0, 0],
        ));

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Assisted);

    assert!(scenario.eventai_speech().is_empty());
    assert_eq!(scenario.casts(), vec![(CREATURE, 61, TARGET)]);
}

#[test]
fn a_combat_action_retries_only_when_its_first_cast_is_refused() {
    let mut cast = native_row(
        611,
        610,
        0,
        EVENT_ON_AGGRO,
        ACTION_CAST,
        100,
        1,
        REPEAT_ONCE,
        [0; 6],
        [62, 0, 0],
    );
    cast.source_flags = SOURCE_FLAG_COMBAT_ACTION;
    let mut say = native_row(
        612,
        610,
        1,
        EVENT_ON_AGGRO,
        ACTION_SAY,
        100,
        1,
        REPEAT_ONCE,
        [0; 6],
        [63, 0, 0],
    );
    say.source_flags = SOURCE_FLAG_COMBAT_ACTION;
    let mut scenario = world(1_000_000)
        .eventai_broadcast(63, "after cast", 0)
        .eventai_row(cast)
        .eventai_row(say)
        .not_ready(62);

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);
    assert!(scenario.casts().is_empty());
    assert!(scenario.eventai_speech().is_empty());
    assert!(scenario.eventai_state(CREATURE, 610).is_none());

    scenario.not_ready.borrow_mut().remove(&62);
    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    assert_eq!(scenario.casts(), vec![(CREATURE, 62, TARGET)]);
    assert_eq!(scenario.eventai_speech().len(), 1);
    assert!(scenario.eventai_state(CREATURE, 610).unwrap().consumed);
}

#[test]
fn a_later_refused_cast_does_not_hold_a_combat_action_open() {
    let mut say = native_row(
        621,
        620,
        0,
        EVENT_ON_AGGRO,
        ACTION_SAY,
        100,
        1,
        REPEAT_ONCE,
        [0; 6],
        [64, 0, 0],
    );
    say.source_flags = SOURCE_FLAG_COMBAT_ACTION;
    let mut cast = native_row(
        622,
        620,
        1,
        EVENT_ON_AGGRO,
        ACTION_CAST,
        100,
        1,
        REPEAT_ONCE,
        [0; 6],
        [65, 0, 0],
    );
    cast.source_flags = SOURCE_FLAG_COMBAT_ACTION;
    let mut scenario = world(1_000_000)
        .eventai_broadcast(64, "first action", 0)
        .eventai_row(say)
        .eventai_row(cast)
        .not_ready(65);

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    assert_eq!(scenario.eventai_speech().len(), 1);
    assert!(scenario.casts().is_empty());
    assert!(scenario.eventai_state(CREATURE, 620).unwrap().consumed);
}

#[test]
fn timed_casts_rearm_from_the_fire_time() {
    let rule_id = 700;
    let mut scenario = world(0).attacking(CREATURE, TARGET).eventai_row(native_row(
        701,
        rule_id,
        0,
        EVENT_TIMED_IN_COMBAT,
        ACTION_CAST,
        100,
        1,
        REPEAT,
        [500, 500, 1_000, 1_000, 0, 0],
        [70, 0, 0],
    ));

    let first = fire(&mut scenario);
    assert_eq!(
        first
            .rows_visited
            .iter()
            .find(|row| row.0 == "eventai")
            .unwrap()
            .1,
        1
    );
    scenario.advance_clock(500_000);
    fire(&mut scenario);
    scenario.advance_clock(500_000);
    fire(&mut scenario);
    scenario.advance_clock(500_000);
    fire(&mut scenario);

    assert_eq!(
        scenario.casts(),
        vec![(CREATURE, 70, TARGET), (CREATURE, 70, TARGET)]
    );
    assert_eq!(
        scenario
            .eventai_state(CREATURE, rule_id)
            .unwrap()
            .next_eligible_ms,
        2_500
    );
}

#[test]
fn a_removed_rule_is_reaped_for_the_creature_evaluated_next() {
    let mut scenario = world(0).attacking(CREATURE, TARGET).eventai_row(native_row(
        801,
        800,
        0,
        EVENT_TIMED_IN_COMBAT,
        ACTION_CAST,
        100,
        1,
        REPEAT,
        [500, 500, 500, 500, 0, 0],
        [80, 0, 0],
    ));
    fire(&mut scenario);
    assert!(scenario.eventai_state(CREATURE, 800).is_some());

    scenario.remove_eventai_rule(800);
    fire(&mut scenario);
    assert!(scenario.eventai_state(CREATURE, 800).is_none());
}

#[test]
fn rule_state_cleanup_does_not_scan_other_creatures() {
    let other = CREATURE + 10;
    let mut scenario = world(0)
        .creature(other, point(1.0))
        .entry(other, ENTRY)
        .attacking(CREATURE, TARGET)
        .attacking(other, TARGET)
        .eventai_row(native_row(
            811,
            810,
            0,
            EVENT_TIMED_IN_COMBAT,
            ACTION_CAST,
            100,
            1,
            REPEAT,
            [500, 500, 500, 500, 0, 0],
            [81, 0, 0],
        ));
    fire(&mut scenario);
    assert!(scenario.eventai_state(CREATURE, 810).is_some());
    assert!(scenario.eventai_state(other, 810).is_some());

    scenario.remove_eventai_rule(810);
    eventai::evaluate(
        &mut scenario,
        EventAiRequest::Edge(EventContext {
            kind: EventKind::OnAggro,
            creature_guid: CREATURE,
            invoker_guid: Some(TARGET),
            event_target_guid: Some(TARGET),
            current_target_guid: Some(TARGET),
            assisted: false,
            now_ms: 0,
        }),
    );

    assert!(scenario.eventai_state(CREATURE, 810).is_none());
    assert!(scenario.eventai_state(other, 810).is_some());
}

#[test]
fn inconsistent_group_metadata_fails_closed_with_a_diagnostic() {
    let mut second = native_row(
        902,
        900,
        1,
        EVENT_ON_AGGRO,
        ACTION_CAST,
        100,
        1,
        REPEAT_ONCE,
        [0; 6],
        [90, 0, 0],
    );
    second.chance_pct = 99;
    let mut scenario = world(0)
        .eventai_row(native_row(
            901,
            900,
            0,
            EVENT_ON_AGGRO,
            ACTION_CAST,
            100,
            1,
            REPEAT_ONCE,
            [0; 6],
            [90, 0, 0],
        ))
        .eventai_row(second);

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    assert!(scenario.casts().is_empty());
    assert_eq!(
        scenario.eventai_diagnostics()[0].kind,
        DiagnosticKind::InconsistentRule
    );
}

#[test]
fn an_unimplemented_action_stays_ready_and_reports_why() {
    let mut scenario = world(0).eventai_row(native_row(
        911,
        910,
        0,
        EVENT_ON_AGGRO,
        ACTION_SUMMON,
        100,
        1,
        REPEAT_ONCE,
        [0; 6],
        [1, 0, 0],
    ));

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    assert!(scenario.eventai_state(CREATURE, 910).is_none());
    assert_eq!(
        scenario.eventai_diagnostics()[0].kind,
        DiagnosticKind::UnsupportedAction
    );
}
