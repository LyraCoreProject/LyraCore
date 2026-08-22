use std::collections::{BTreeMap, HashSet};

use spacetimedb::{ReducerContext, Table};

use super::{
    normalized_revision, CastInstruction, CreatureAiBroadcastText, CreatureAiDefinition,
    CreatureInstruction, EventAiRule, EventAiSubject, EventCondition, ExecutionPolicy,
    InstructionSelection, InstructionTarget, PhaseSet, RecurrencePolicy, SpeakInstruction,
    SpeechMode, TimeWindow,
};
use crate::{
    game_creature_ai_broadcast_text, game_creature_ai_definition, game_creature_ai_rule_state,
};

const FIRST_RULE_ID: u64 = 5_099_000;
const FIRST_TEXT_ID: u32 = 5_099_100;
const BARKS: [(u32, SpeechMode, &str); 3] = [
    (6, SpeechMode::Say, "Grrr!"),
    (116, SpeechMode::Yell, "Intruder!"),
    (448, SpeechMode::Yell, "Roar!"),
];
const BOSS_CASTS: [(u32, u32, u32, u32, u32, u32); 6] = [
    (644, 6304, 5_000, 9_000, 8_000, 14_000),
    (642, 3603, 4_000, 8_000, 9_000, 14_000),
    (642, 7399, 10_000, 15_000, 18_000, 25_000),
    (1763, 5213, 4_000, 7_000, 9_000, 13_000),
    (1763, 5159, 8_000, 12_000, 14_000, 18_000),
    (646, 6432, 6_000, 10_000, 12_000, 17_000),
];

/// Replace every definition owned by the small development fixture catalogue.
pub(crate) fn seed_on_aggro_fixtures(ctx: &ReducerContext) {
    let definitions = ctx.db.game_creature_ai_definition();
    let mut cleared = HashSet::new();
    for definition in definitions
        .iter()
        .filter(|definition| is_seeded_entry(definition.creature_entry))
        .collect::<Vec<_>>()
    {
        cleared.extend(definition.rules.iter().map(|rule| rule.source_rule_id));
        definitions.id().delete(definition.id);
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
    let mut by_entry: BTreeMap<u32, Vec<EventAiRule>> = BTreeMap::new();
    for (offset, (entry, mode, message)) in BARKS.into_iter().enumerate() {
        let rule_id = FIRST_RULE_ID + offset as u64;
        let text_id = FIRST_TEXT_ID + offset as u32;
        texts.id().delete(text_id);
        texts.insert(CreatureAiBroadcastText {
            id: text_id,
            male_text: message.to_string(),
            female_text: message.to_string(),
            chat_type: u8::from(mode == SpeechMode::Yell),
            language_id: 0,
            emote_delay_1_ms: 0,
            emote_id_1: 0,
            emote_delay_2_ms: 0,
            emote_id_2: 0,
            emote_delay_3_ms: 0,
            emote_id_3: 0,
        });
        by_entry.entry(entry).or_default().push(EventAiRule {
            source_rule_id: rule_id,
            event: EventCondition::OnAggro,
            chance_pct: 100,
            allowed_phases: PhaseSet { bits: u32::MAX },
            recurrence: RecurrencePolicy::Once,
            selection: InstructionSelection::All,
            execution: ExecutionPolicy::Ordinary,
            instructions: vec![CreatureInstruction::Speak(SpeakInstruction {
                mode,
                broadcast_ids: vec![text_id],
                legacy_text: String::new(),
                target: InstructionTarget::SelfActor,
            })],
        });
    }

    for (offset, (entry, spell, initial_min, initial_max, repeat_min, repeat_max)) in
        BOSS_CASTS.into_iter().enumerate()
    {
        by_entry.entry(entry).or_default().push(EventAiRule {
            source_rule_id: FIRST_RULE_ID + 3 + offset as u64,
            event: EventCondition::TimedInCombat(TimeWindow {
                min_ms: initial_min,
                max_ms: initial_max,
            }),
            chance_pct: 100,
            allowed_phases: PhaseSet { bits: u32::MAX },
            recurrence: RecurrencePolicy::Repeat(TimeWindow {
                min_ms: repeat_min,
                max_ms: repeat_max,
            }),
            selection: InstructionSelection::All,
            execution: ExecutionPolicy::Ordinary,
            instructions: vec![CreatureInstruction::Cast(CastInstruction {
                spell_id: spell,
                target: InstructionTarget::CurrentOpponent,
                interrupt_previous: false,
                triggered: false,
                aura_absent: false,
                character_only: false,
                target_must_be_casting: false,
            })],
        });
    }

    for (entry, mut rules) in by_entry {
        rules.sort_by_key(|rule| rule.source_rule_id);
        let revision = normalized_revision(EventAiSubject::Entry(entry), &rules);
        definitions.insert(CreatureAiDefinition {
            id: 0,
            creature_entry: entry,
            creature_guid: 0,
            definition_revision: revision.value,
            rules,
        });
    }
}

fn is_seeded_entry(entry: u32) -> bool {
    BARKS.iter().any(|&(seeded, ..)| seeded == entry)
        || BOSS_CASTS.iter().any(|&(seeded, ..)| seeded == entry)
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

    #[test]
    fn every_seeded_entry_is_cleared_whoever_wrote_its_definition() {
        assert!(is_seeded_entry(644));
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
            body.contains("is_seeded_entry(definition.creature_entry)"),
            "the seed no longer clears every definition owned by a seeded entry"
        );
    }

    #[test]
    fn fixture_rule_ids_stay_in_the_reserved_catalogue() {
        for id in FIRST_RULE_ID..=FIRST_RULE_ID + 8 {
            assert!((5_090_000..=5_099_999).contains(&id));
        }
        assert!((5_090_000..=5_099_999).contains(&u64::from(FIRST_TEXT_ID)));
    }
}
