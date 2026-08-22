use std::collections::HashSet;

use spacetimedb::{ReducerContext, Table};

use super::{
    effective_rule_id, CreatureAiBroadcastText, CreatureAiEvent, ACTION_CAST, ACTION_SAY,
    ACTION_YELL, EVENT_ON_AGGRO, EVENT_TIMED_IN_COMBAT, REPEAT, REPEAT_ONCE, TARGET_CURRENT,
};
use crate::{game_creature_ai_broadcast_text, game_creature_ai_event, game_creature_ai_rule_state};

const FIRST_RULE_ID: u64 = 5_099_000;
const FIRST_TEXT_ID: u32 = 5_099_100;
const BARKS: [(u32, u8, &str); 3] = [
    (6, ACTION_SAY, "Grrr!"),
    (116, ACTION_YELL, "Intruder!"),
    (448, ACTION_YELL, "Roar!"),
];
const BOSS_CASTS: [(u32, u32, u32, u32, u32, u32); 6] = [
    (644, 6304, 5_000, 9_000, 8_000, 14_000),
    (642, 3603, 4_000, 8_000, 9_000, 14_000),
    (642, 7399, 10_000, 15_000, 18_000, 25_000),
    (1763, 5213, 4_000, 7_000, 9_000, 13_000),
    (1763, 5159, 8_000, 12_000, 14_000, 18_000),
    (646, 6432, 6_000, 10_000, 12_000, 17_000),
];

/// Write the fixture barks and boss casts, replacing every rule a seeded entry already has.
/// Reseeding on top of an imported world is the point: entry 644 must end with ONE Slam rule, not
/// the fixture's plus the importer's on independent timers. Live rule state for the cleared rules
/// goes with them, so no creature keeps a window belonging to a rule that no longer exists.
pub(crate) fn seed_on_aggro_fixtures(ctx: &ReducerContext) {
    let rules = ctx.db.game_creature_ai_event();
    let mut cleared: HashSet<u64> = HashSet::new();
    for row in rules
        .iter()
        .filter(|row| is_seeded_entry(row.creature_entry))
        .collect::<Vec<_>>()
    {
        cleared.insert(effective_rule_id(&row));
        rules.id().delete(row.id);
    }
    let rule_state = ctx.db.game_creature_ai_rule_state();
    for state in rule_state
        .iter()
        .filter(|state| cleared.contains(&state.source_rule_id))
        .collect::<Vec<_>>()
    {
        rule_state.id().delete(state.id);
    }

    let texts = ctx.db.game_creature_ai_broadcast_text();
    for (offset, (entry, action, message)) in BARKS.into_iter().enumerate() {
        let rule_id = FIRST_RULE_ID + offset as u64;
        let text_id = FIRST_TEXT_ID + offset as u32;
        texts.id().delete(text_id);
        texts.insert(CreatureAiBroadcastText {
            id: text_id,
            male_text: message.to_string(),
            female_text: message.to_string(),
            chat_type: if action == ACTION_YELL { 1 } else { 0 },
            language_id: 0,
            emote_delay_1_ms: 0,
            emote_id_1: 0,
            emote_delay_2_ms: 0,
            emote_id_2: 0,
            emote_delay_3_ms: 0,
            emote_id_3: 0,
        });
        rules.insert(native_row(
            rule_id,
            entry,
            EVENT_ON_AGGRO,
            action,
            REPEAT_ONCE,
            [0; 6],
            [text_id, 0, 0],
        ));
    }

    for (offset, (entry, spell, initial_min, initial_max, repeat_min, repeat_max)) in
        BOSS_CASTS.into_iter().enumerate()
    {
        let rule_id = FIRST_RULE_ID + 3 + offset as u64;
        rules.insert(native_row(
            rule_id,
            entry,
            EVENT_TIMED_IN_COMBAT,
            ACTION_CAST,
            REPEAT,
            [initial_min, initial_max, repeat_min, repeat_max, 0, 0],
            [spell, 0, 0],
        ));
    }
}

/// Does this fixture set own every rule for `entry`? True for the barks and the boss casts alike,
/// whatever wrote them: a hand-seeded row, a row from the previous schema, or an imported one.
fn is_seeded_entry(entry: u32) -> bool {
    BARKS.iter().any(|&(seeded, ..)| seeded == entry)
        || BOSS_CASTS.iter().any(|&(seeded, ..)| seeded == entry)
}

fn native_row(
    id: u64,
    creature_entry: u32,
    event_type: u8,
    action_type: u8,
    repeat_policy: u8,
    event_params: [u32; 6],
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
        source_rule_id: id,
        action_order: 0,
        creature_guid: 0,
        chance_pct: 100,
        allowed_phase_mask: u32::MAX,
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

#[cfg(feature = "debug_reducers")]
#[spacetimedb::reducer]
pub fn debug_seed_creature_ai_fixtures(ctx: &ReducerContext) {
    seed_on_aggro_fixtures(ctx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_scan::code_of;

    /// Reseeding on top of an imported world must leave ONE Slam rule for Rhahk'Zor, not the
    /// fixture's plus the importer's on independent timers. The clear keys on the ENTRY, so it
    /// reaches an imported row whatever id or source rule it carries.
    #[test]
    fn every_seeded_entry_is_cleared_whoever_wrote_its_rules() {
        assert!(is_seeded_entry(644)); // Rhahk'Zor, whose Slam the importer also writes
        for (entry, ..) in BARKS {
            assert!(is_seeded_entry(entry));
        }
        for (entry, ..) in BOSS_CASTS {
            assert!(is_seeded_entry(entry));
        }
        assert!(!is_seeded_entry(1));

        let body = code_of(
            include_str!("fixtures.rs"),
            "pub(crate) fn seed_on_aggro_fixtures(ctx: &ReducerContext) {",
        );
        assert!(
            body.contains("is_seeded_entry(row.creature_entry)"),
            "the seed no longer clears every rule a seeded entry already has"
        );
    }

    #[test]
    fn static_fixture_ids_are_explicit_and_reserved() {
        for id in FIRST_RULE_ID..=FIRST_RULE_ID + 8 {
            assert!((5_090_000..=5_099_999).contains(&id));
            let row = native_row(
                id,
                1,
                EVENT_ON_AGGRO,
                ACTION_SAY,
                REPEAT_ONCE,
                [0; 6],
                [FIRST_TEXT_ID, 0, 0],
            );
            assert_eq!(row.id, id);
            assert_eq!(row.source_rule_id, id);
        }
        assert!((5_090_000..=5_099_999).contains(&u64::from(FIRST_TEXT_ID)));
    }
}
