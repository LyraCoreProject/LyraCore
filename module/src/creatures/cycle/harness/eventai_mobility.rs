//! EventAI summon and ranged-posture scenarios.

use super::*;
use crate::creatures::eventai::*;

const CREATURE: u64 = 8_201;
const TARGET: u64 = 8_202;
const ENTRY: u32 = 920;
const SUMMON_ENTRY: u32 = 921;

fn point(x: f32, y: f32) -> Point {
    Point { x, y, z: 0.0 }
}

fn scope() -> TickScope {
    TickScope::CatchAll {
        dedicated: HashSet::new(),
    }
}

fn row(
    id: u64,
    rule_id: u64,
    order: u8,
    creature_entry: u32,
    event_type: u8,
    action_type: u8,
    action_params: [u32; 3],
) -> CreatureAiEvent {
    CreatureAiEvent {
        id,
        creature_entry,
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
        repeat_policy: REPEAT_ONCE,
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

fn world() -> Scenario {
    Scenario::new(1_000_000)
        .creature(CREATURE, point(0.0, 0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(20.0, 0.0))
}

fn fire(scenario: &mut Scenario, sense: bool) {
    let tick = scenario.tick(sense, scope());
    run_cycle(scenario, tick);
}

#[test]
fn a_fixed_ranged_angle_uses_the_existing_chase_leg() {
    let mut scenario = world().eventai_row(row(
        509_0300,
        509_0300,
        0,
        ENTRY,
        EVENT_ON_AGGRO,
        ACTION_SET_RANGED_POSTURE,
        [10, 90, 0],
    ));

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);
    fire(&mut scenario, false);

    let effects = scenario.effects();
    assert_eq!(effects.len(), 1);
    assert!((effects[0].dest.x - 20.0).abs() < 0.01);
    assert!((effects[0].dest.y - 10.0).abs() < 0.01);
    assert!(effects[0].run);
    assert_eq!(
        PursuitSink::authored_ranged_posture(&scenario, CREATURE),
        Some((10.0, std::f32::consts::FRAC_PI_2))
    );

    EngageSink::leave_combat(&mut scenario, CREATURE);

    assert_eq!(
        PursuitSink::authored_ranged_posture(&scenario, CREATURE),
        None
    );
}

#[test]
fn a_random_source_action_selects_one_fixed_ranged_angle() {
    let mut left = row(
        509_0301,
        509_0301,
        0,
        ENTRY,
        EVENT_ON_AGGRO,
        ACTION_SET_RANGED_POSTURE,
        [10, 90, 0],
    );
    left.source_flags = SOURCE_FLAG_RANDOM_ACTION;
    let mut right = row(
        509_0302,
        509_0301,
        1,
        ENTRY,
        EVENT_ON_AGGRO,
        ACTION_SET_RANGED_POSTURE,
        [10, (-90_i32) as u32, 0],
    );
    right.source_flags = SOURCE_FLAG_RANDOM_ACTION;
    let mut scenario = world().eventai_row(left).eventai_row(right).rolls([1]);

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);
    fire(&mut scenario, false);

    let effects = scenario.effects();
    assert_eq!(effects.len(), 1);
    assert!((effects[0].dest.x - 20.0).abs() < 0.01);
    assert!((effects[0].dest.y + 10.0).abs() < 0.01);
}

#[test]
fn zero_distance_restores_the_unchanged_melee_chase() {
    let mut expected = world().attacking(CREATURE, TARGET);
    fire(&mut expected, false);

    let mut caster = world().attacking(CREATURE, TARGET).caster(CREATURE, 30.0);
    fire(&mut caster, false);

    let mut restored = world().caster(CREATURE, 30.0).eventai_row(row(
        509_0303,
        509_0303,
        0,
        ENTRY,
        EVENT_ON_AGGRO,
        ACTION_SET_RANGED_POSTURE,
        [0, 0, 0],
    ));
    EngageSink::engage(&mut restored, CREATURE, TARGET, Pull::Noticed);
    fire(&mut restored, false);

    assert!(caster.effects().is_empty());
    assert_eq!(restored.effects(), expected.effects());
}

#[test]
fn a_summon_inherits_partition_fires_spawn_once_and_engages() {
    let mut spawn_speech = row(
        509_0305,
        509_0305,
        0,
        SUMMON_ENTRY,
        EVENT_ON_SPAWN,
        ACTION_SAY,
        [77, 0, 0],
    );
    spawn_speech.target_policy = TARGET_SELF;
    let mut summon = row(
        509_0304,
        509_0304,
        0,
        ENTRY,
        EVENT_ON_AGGRO,
        ACTION_SUMMON,
        [SUMMON_ENTRY, 1, 7],
    );
    summon.target_policy = TARGET_CURRENT;
    let mut scenario = world()
        .in_instance(CREATURE, 42)
        .tweak_player(TARGET, |target| target.instance_id = 42)
        .eventai_template(SUMMON_ENTRY)
        .eventai_summon(7, point(5.0, 6.0), 1.25, 1_000)
        .eventai_broadcast(77, "ready", 0)
        .eventai_row(summon)
        .eventai_row(spawn_speech);

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);

    let summoned_guids = scenario.eventai_summoned_guids();
    let [summoned] = summoned_guids.as_slice() else {
        panic!("expected one summon");
    };
    let creature = scenario.creatures.borrow()[summoned];
    assert_eq!(creature.entry, SUMMON_ENTRY);
    assert_eq!(creature.at, point(5.0, 6.0));
    assert_eq!(creature.orientation, 1.25);
    assert_eq!((creature.map_id, creature.instance_id), (MAP, 42));
    assert!(scenario
        .fights
        .borrow()
        .iter()
        .any(|fight| fight.attacker == *summoned && fight.victim == TARGET));
    assert_eq!(
        scenario.eventai_speech(),
        vec![(*summoned, 0, "ready".to_string())]
    );
}

#[test]
fn summon_lifetime_waits_out_combat_then_uses_full_cleanup() {
    let mut summon = row(
        509_0306,
        509_0306,
        0,
        ENTRY,
        EVENT_ON_AGGRO,
        ACTION_SUMMON,
        [SUMMON_ENTRY, 1, 8],
    );
    summon.target_policy = TARGET_CURRENT;
    let spawn_phase = row(
        509_0308,
        509_0308,
        0,
        SUMMON_ENTRY,
        EVENT_ON_SPAWN,
        ACTION_SET_PHASE,
        [2, 0, 0],
    );
    let mut scenario = world()
        .eventai_template(SUMMON_ENTRY)
        .eventai_summon(8, point(5.0, 0.0), 0.0, 1_000)
        .eventai_row(summon)
        .eventai_row(spawn_phase);
    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);
    let summoned = scenario.eventai_summoned_guids()[0];
    assert!(scenario.eventai_rule_state.borrow().contains_key(&summoned));
    assert!(scenario
        .eventai_creature_state
        .borrow()
        .contains_key(&summoned));

    scenario.advance_clock(2_000_000);
    fire(&mut scenario, true);
    assert!(scenario.creatures.borrow().contains_key(&summoned));

    scenario
        .fights
        .borrow_mut()
        .retain(|fight| fight.attacker != summoned && fight.victim != summoned);
    scenario.advance_clock(1_000_000);
    fire(&mut scenario, true);

    assert!(!scenario.creatures.borrow().contains_key(&summoned));
    assert!(!scenario
        .eventai_summon_expiry
        .borrow()
        .contains_key(&summoned));
    assert!(!scenario.eventai_rule_state.borrow().contains_key(&summoned));
    assert!(!scenario
        .eventai_creature_state
        .borrow()
        .contains_key(&summoned));
}

#[test]
fn missing_summon_catalogue_or_template_fails_without_a_partial_creature() {
    let summon = row(
        509_0307,
        509_0307,
        0,
        ENTRY,
        EVENT_ON_AGGRO,
        ACTION_SUMMON,
        [SUMMON_ENTRY, 1, 9],
    );
    let mut missing_location = world()
        .eventai_template(SUMMON_ENTRY)
        .eventai_row(summon.clone());
    EngageSink::engage(&mut missing_location, CREATURE, TARGET, Pull::Noticed);
    assert!(missing_location.eventai_summoned_guids().is_empty());

    let mut missing_template = world()
        .eventai_summon(9, point(5.0, 0.0), 0.0, 1_000)
        .eventai_row(summon);
    EngageSink::engage(&mut missing_template, CREATURE, TARGET, Pull::Noticed);
    assert!(missing_template.eventai_summoned_guids().is_empty());
}

#[test]
fn a_creature_standing_on_its_authored_hold_point_turns_to_its_victim() {
    // Placed exactly where the 10 yd / 90° posture holds it, looking the other way — the shape a
    // victim who ran a quarter circle around it leaves behind.
    let mut scenario = Scenario::new(1_000_000)
        .creature(CREATURE, point(20.0, 10.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(20.0, 0.0))
        .facing(CREATURE, std::f32::consts::FRAC_PI_2)
        .eventai_row(row(
            509_0309,
            509_0309,
            0,
            ENTRY,
            EVENT_ON_AGGRO,
            ACTION_SET_RANGED_POSTURE,
            [10, 90, 0],
        ));

    EngageSink::engage(&mut scenario, CREATURE, TARGET, Pull::Noticed);
    fire(&mut scenario, false);

    let effects = scenario.effects();
    assert_eq!(
        effects
            .iter()
            .map(|effect| (effect.dur_ms, effect.facing))
            .collect::<Vec<_>>(),
        [(0, true)],
        "arriving at an authored hold point is an arrival like reaching melee: a creature that only \
         ever stops there renders with its back to what it is shooting at"
    );
    assert!((effects[0].facing_angle + std::f32::consts::FRAC_PI_2).abs() < 0.01);
    assert_eq!(
        scenario.at(CREATURE).at,
        point(20.0, 10.0),
        "the turn must not move it off the hold point"
    );
}
