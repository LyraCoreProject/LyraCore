use spacetimedb::{ReducerContext, Table};

use super::{
    CreatureAiBroadcastText, CreatureAiEvent, ACTION_CAST, ACTION_SAY, ACTION_YELL, EVENT_ON_AGGRO,
    EVENT_TIMED_IN_COMBAT, REPEAT, REPEAT_ONCE, TARGET_CURRENT,
};
use crate::{game_creature_ai_broadcast_text, game_creature_ai_event};

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

pub(crate) fn seed_on_aggro_fixtures(ctx: &ReducerContext) {
    let rules = ctx.db.game_creature_ai_event();
    for id in FIRST_RULE_ID..=FIRST_RULE_ID + 8 {
        rules.id().delete(id);
    }
    // Remove the auto-increment fixture rows written by the previous schema without touching
    // imported native rules.
    for row in rules.iter().filter(is_previous_fixture).collect::<Vec<_>>() {
        rules.id().delete(row.id);
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

fn is_previous_fixture(row: &CreatureAiEvent) -> bool {
    if row.source_rule_id != 0 || row.creature_guid != 0 {
        return false;
    }
    BARKS.iter().any(|&(entry, action, message)| {
        row.creature_entry == entry
            && row.event_type == EVENT_ON_AGGRO
            && row.action_type == action
            && row.text == message
            && row.spell_id == 0
    }) || BOSS_CASTS.iter().any(
        |&(entry, spell, initial_min, initial_max, repeat_min, repeat_max)| {
            row.creature_entry == entry
                && row.event_type == EVENT_TIMED_IN_COMBAT
                && row.action_type == ACTION_CAST
                && row.spell_id == spell
                && row.initial_min_ms == initial_min
                && row.initial_max_ms == initial_max
                && row.repeat_min_ms == repeat_min
                && row.repeat_max_ms == repeat_max
        },
    )
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
