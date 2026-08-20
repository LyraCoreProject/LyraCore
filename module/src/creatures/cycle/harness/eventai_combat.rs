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
        .attacking(CREATURE, TARGET)
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
        .eventai_row(action);

    fire(&mut scenario);

    assert!(scenario.eventai_diagnostics().is_empty());
    assert_eq!(scenario.casts(), vec![(CREATURE, 103, FRIEND)]);
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
    assert!(scenario.eventai_state(CREATURE, 509_0108).is_none());

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
        policies
            .iter()
            .enumerate()
            .map(|(order, (_, target))| (CREATURE, 200 + order as u32, *target))
            .collect::<Vec<_>>()
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
fn triggered_and_non_interrupting_pending_casts_fail_closed() {
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

    assert!(scenario.casts().is_empty());
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
    assert_eq!(
        flee_legs.len(),
        2,
        "a later flee in the same fight must write a second leg away from the victim"
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
fn eventai_cast_rules_suppress_flat_casts_without_changing_unscripted_creatures() {
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
fn an_authored_cast_rule_sends_its_caster_to_melee_and_replaces_the_flat_rotation() {
    let mut scenario = Scenario::new(0)
        .creature(CREATURE, point(0.0))
        .entry(CREATURE, ENTRY)
        .player(TARGET, point(20.0))
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
            [0, 30, 0, 0, 0, 0],
            [113, 0, 0],
        ));

    fire(&mut scenario);

    assert!(
        scenario.casts().is_empty(),
        "the flat rotation is off, so nothing may be cast before the authored band opens"
    );
    assert!(
        scenario.has_leg(CREATURE),
        "a creature whose casting the script owns holds at no rotation range: it closes to melee \
         and swings between authored casts, instead of standing at 30 yards casting nothing"
    );

    scenario = scenario.hurt(CREATURE, 30);
    fire(&mut scenario);

    assert_eq!(scenario.casts(), vec![(CREATURE, 113, TARGET)]);
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
