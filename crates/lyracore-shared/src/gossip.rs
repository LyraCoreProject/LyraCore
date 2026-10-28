use crate::constants::gossip_condition as encoded;

/// See docs/data-ingestion.md when changing the stable stored condition codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConditionKind {
    Not,
    Any,
    All,
    Always,
    Team,
    QuestRewarded,
    QuestTaken,
    RaceClass,
    Level,
    QuestAbsent,
    Unsupported(i32),
}

impl ConditionKind {
    pub fn from_code(code: i32) -> Self {
        match code {
            -3 => Self::Not,
            -2 => Self::Any,
            -1 => Self::All,
            0 => Self::Always,
            6 => Self::Team,
            8 => Self::QuestRewarded,
            9 => Self::QuestTaken,
            14 => Self::RaceClass,
            15 => Self::Level,
            22 => Self::QuestAbsent,
            other => Self::Unsupported(other),
        }
    }

    pub fn code(self) -> i32 {
        match self {
            Self::Not => -3,
            Self::Any => -2,
            Self::All => -1,
            Self::Always => 0,
            Self::Team => 6,
            Self::QuestRewarded => 8,
            Self::QuestTaken => 9,
            Self::RaceClass => 14,
            Self::Level => 15,
            Self::QuestAbsent => 22,
            Self::Unsupported(code) => code,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Condition {
    pub kind: ConditionKind,
    pub values: [u32; 4],
    pub flags: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestState {
    /// The quest template does not exist.
    Missing,
    Absent,
    Incomplete,
    Complete,
    Failed,
    Rewarded,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CharacterFacts {
    pub level: u32,
    pub race: u8,
    pub class: u8,
}

/// An unknown result cannot become eligible through NOT, reverse-result flags, or OR.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    Missing(u32),
    MissingQuest(u32),
    Cycle(u32),
    Unsupported(i32),
    Operands(i32),
    Flags(u8),
    Limit,
}

/// Only `Ok(true)` permits showing or executing an option.
pub fn evaluate_option(
    kind: u32,
    value1: u32,
    value2: u32,
    character: CharacterFacts,
    quest: impl Fn(u32) -> QuestState,
    condition: impl Fn(u32) -> Option<Condition>,
) -> Result<bool, Unavailable> {
    let condition_kind = match kind {
        encoded::NONE => return Ok(true),
        encoded::QUEST_TAKEN => ConditionKind::QuestTaken,
        encoded::QUEST_REWARDED => ConditionKind::QuestRewarded,
        encoded::NEVER => return Ok(false),
        encoded::LEVEL => ConditionKind::Level,
        encoded::TEAM => ConditionKind::Team,
        encoded::RACE_CLASS => ConditionKind::RaceClass,
        encoded::QUEST_NONE => ConditionKind::QuestAbsent,
        encoded::REFERENCE => {
            return evaluate_reference(value1, character, quest, condition);
        }
        _ => return Err(Unavailable::Unsupported(kind as i32)),
    };
    Evaluation::new(character, &quest, &condition).row(Condition {
        kind: condition_kind,
        values: [value1, value2, 0, 0],
        flags: 0,
    })
}

/// Missing, cyclic, or excessive references return `Err`.
pub fn evaluate_reference(
    id: u32,
    character: CharacterFacts,
    quest: impl Fn(u32) -> QuestState,
    condition: impl Fn(u32) -> Option<Condition>,
) -> Result<bool, Unavailable> {
    Evaluation::new(character, &quest, &condition).reference(id)
}

struct Evaluation<'a, Q, C> {
    character: CharacterFacts,
    quest: &'a Q,
    condition: &'a C,
    path: Vec<u32>,
    remaining: usize,
}

impl<'a, Q, C> Evaluation<'a, Q, C>
where
    Q: Fn(u32) -> QuestState,
    C: Fn(u32) -> Option<Condition>,
{
    fn new(character: CharacterFacts, quest: &'a Q, condition: &'a C) -> Self {
        Self {
            character,
            quest,
            condition,
            path: Vec::new(),
            remaining: 256,
        }
    }

    fn reference(&mut self, id: u32) -> Result<bool, Unavailable> {
        if self.path.contains(&id) {
            return Err(Unavailable::Cycle(id));
        }
        if self.path.len() >= 32 || self.remaining == 0 {
            return Err(Unavailable::Limit);
        }
        self.remaining -= 1;
        let row = (self.condition)(id).ok_or(Unavailable::Missing(id))?;
        self.path.push(id);
        let result = self.row(row);
        self.path.pop();
        result
    }

    fn row(&mut self, row: Condition) -> Result<bool, Unavailable> {
        if row.flags & !1 != 0 {
            return Err(Unavailable::Flags(row.flags));
        }
        let [a, b, c, d] = row.values;
        if !matches!(
            row.kind,
            ConditionKind::Not | ConditionKind::Any | ConditionKind::All
        ) && (c != 0 || d != 0)
        {
            return Err(Unavailable::Operands(row.kind.code()));
        }
        let result = match row.kind {
            ConditionKind::Not if b == 0 && c == 0 && d == 0 => !self.reference(a)?,
            ConditionKind::Any | ConditionKind::All => {
                let mut result = row.kind == ConditionKind::All;
                for id in [a, b]
                    .into_iter()
                    .chain([c, d].into_iter().filter(|id| *id != 0))
                {
                    // Evaluate every operand: a true branch cannot conceal an unsupported sibling.
                    let child = self.reference(id)?;
                    result = if row.kind == ConditionKind::All {
                        result && child
                    } else {
                        result || child
                    };
                }
                result
            }
            ConditionKind::Always if a == 0 && b == 0 => true,
            ConditionKind::Team if b == 0 && matches!(a, 67 | 469) => {
                if !matches!(self.character.race, 1..=8) {
                    return Err(Unavailable::Operands(row.kind.code()));
                }
                crate::faction::team_for_race(self.character.race) == a
            }
            ConditionKind::QuestRewarded if a != 0 && b == 0 => {
                self.quest_state(a)? == QuestState::Rewarded
            }
            ConditionKind::QuestTaken if a != 0 && b <= 2 => matches!(
                (self.quest_state(a)?, b),
                (QuestState::Incomplete, 0 | 1) | (QuestState::Complete, 0 | 2)
            ),
            ConditionKind::RaceClass => {
                // Each nonzero mask must contain a playable race or class.
                if (a == 0 && b == 0) || (a != 0 && a & 0xff == 0) || (b != 0 && b & 0x5df == 0) {
                    return Err(Unavailable::Operands(row.kind.code()));
                }
                let race = self
                    .character
                    .race
                    .checked_sub(1)
                    .and_then(|v| 1u32.checked_shl(v.into()));
                let class = self
                    .character
                    .class
                    .checked_sub(1)
                    .and_then(|v| 1u32.checked_shl(v.into()));
                let (Some(race), Some(class)) = (race, class) else {
                    return Err(Unavailable::Operands(row.kind.code()));
                };
                (a == 0 || race & a != 0) && (b == 0 || class & b != 0)
            }
            ConditionKind::Level if a > 0 && a <= 60 && b <= 2 => match b {
                0 => self.character.level == a,
                1 => self.character.level >= a,
                _ => self.character.level <= a,
            },
            ConditionKind::QuestAbsent if a != 0 && b == 0 => {
                matches!(
                    self.quest_state(a)?,
                    QuestState::Absent | QuestState::Failed
                )
            }
            ConditionKind::Unsupported(kind) => return Err(Unavailable::Unsupported(kind)),
            ConditionKind::Not
            | ConditionKind::Always
            | ConditionKind::Team
            | ConditionKind::QuestRewarded
            | ConditionKind::QuestTaken
            | ConditionKind::Level
            | ConditionKind::QuestAbsent => return Err(Unavailable::Operands(row.kind.code())),
        };
        Ok(if row.flags & 1 != 0 { !result } else { result })
    }

    fn quest_state(&self, id: u32) -> Result<QuestState, Unavailable> {
        match (self.quest)(id) {
            QuestState::Missing => Err(Unavailable::MissingQuest(id)),
            state => Ok(state),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const HUMAN_WARRIOR: CharacterFacts = CharacterFacts {
        level: 20,
        race: 1,
        class: 1,
    };

    fn row(kind: ConditionKind, a: u32, b: u32) -> Condition {
        Condition {
            kind,
            values: [a, b, 0, 0],
            flags: 0,
        }
    }

    #[test]
    fn stored_codes_keep_existing_realm_meanings() {
        for (code, kind) in [
            (-3, ConditionKind::Not),
            (-2, ConditionKind::Any),
            (-1, ConditionKind::All),
            (0, ConditionKind::Always),
            (6, ConditionKind::Team),
            (8, ConditionKind::QuestRewarded),
            (9, ConditionKind::QuestTaken),
            (14, ConditionKind::RaceClass),
            (15, ConditionKind::Level),
            (22, ConditionKind::QuestAbsent),
            (99, ConditionKind::Unsupported(99)),
            (i32::MIN, ConditionKind::Unsupported(i32::MIN)),
        ] {
            assert_eq!(ConditionKind::from_code(code), kind);
            assert_eq!(kind.code(), code);
        }
    }

    #[test]
    fn quest_taken_distinguishes_incomplete_complete_failed_and_rewarded() {
        for (state, expected) in [
            (QuestState::Absent, [false, false, false]),
            (QuestState::Incomplete, [true, true, false]),
            (QuestState::Complete, [true, false, true]),
            (QuestState::Failed, [false, false, false]),
            (QuestState::Rewarded, [false, false, false]),
        ] {
            for (operand, expected) in expected.into_iter().enumerate() {
                assert_eq!(
                    evaluate_option(
                        encoded::QUEST_TAKEN,
                        777,
                        operand as u32,
                        HUMAN_WARRIOR,
                        |_| state,
                        |_| None
                    ),
                    Ok(expected)
                );
            }
            assert_eq!(
                evaluate_option(
                    encoded::QUEST_REWARDED,
                    777,
                    0,
                    HUMAN_WARRIOR,
                    |_| state,
                    |_| None
                ),
                Ok(state == QuestState::Rewarded)
            );
        }
    }

    #[test]
    fn level_team_and_race_class_read_both_operands() {
        for (kind, a, b, expected) in [
            (encoded::LEVEL, 20, 0, true),
            (encoded::LEVEL, 21, 0, false),
            (encoded::LEVEL, 19, 1, true),
            (encoded::LEVEL, 21, 1, false),
            (encoded::LEVEL, 21, 2, true),
            (encoded::LEVEL, 19, 2, false),
            (encoded::TEAM, 469, 0, true),
            (encoded::TEAM, 67, 0, false),
            (encoded::RACE_CLASS, 1, 1, true),
            (encoded::RACE_CLASS, 2, 1, false),
            (encoded::RACE_CLASS, 1, 2, false),
            (encoded::RACE_CLASS, 0, 1, true),
            (encoded::RACE_CLASS, 1, 0, true),
            (encoded::RACE_CLASS, 0x101, 0x21, true),
        ] {
            assert_eq!(
                evaluate_option(kind, a, b, HUMAN_WARRIOR, |_| QuestState::Absent, |_| None),
                Ok(expected)
            );
        }
    }

    #[test]
    fn compound_references_and_reverse_flags_preserve_boolean_semantics() {
        let mut rows = HashMap::from([
            (1, row(ConditionKind::Level, 20, 0)),
            (2, row(ConditionKind::QuestRewarded, 777, 0)),
            (3, row(ConditionKind::Not, 2, 0)),
            (4, row(ConditionKind::All, 1, 3)),
            (5, row(ConditionKind::Any, 1, 2)),
        ]);
        let eval = |id, rows: &HashMap<u32, Condition>| {
            evaluate_reference(
                id,
                HUMAN_WARRIOR,
                |_| QuestState::Absent,
                |id| rows.get(&id).copied(),
            )
        };
        assert_eq!(eval(2, &rows), Ok(false));
        assert_eq!(eval(3, &rows), Ok(true));
        assert_eq!(eval(4, &rows), Ok(true));
        assert_eq!(eval(5, &rows), Ok(true));
        rows.get_mut(&4).unwrap().flags = 1;
        assert_eq!(eval(4, &rows), Ok(false));
    }

    #[test]
    fn unsupported_missing_cyclic_and_invalid_conditions_stay_unavailable_under_negation() {
        for bad in [
            row(ConditionKind::Unsupported(99), 0, 0),
            row(ConditionKind::Not, 99, 0),
            row(ConditionKind::Not, 2, 0),
            row(ConditionKind::QuestTaken, 777, 3),
        ] {
            let rows = HashMap::from([
                (1, row(ConditionKind::Not, 2, 0)),
                (2, bad),
                (3, row(ConditionKind::Always, 0, 0)),
                (4, row(ConditionKind::Any, 3, 1)),
            ]);
            for id in [1, 4] {
                assert!(evaluate_reference(
                    id,
                    HUMAN_WARRIOR,
                    |_| QuestState::Absent,
                    |id| rows.get(&id).copied()
                )
                .is_err());
            }
            let mut reverse = bad;
            reverse.flags = 1;
            assert!(evaluate_reference(
                2,
                HUMAN_WARRIOR,
                |_| QuestState::Absent,
                |id| if id == 2 { Some(reverse) } else { None }
            )
            .is_err());
        }
    }

    #[test]
    fn work_is_bounded_even_when_an_acyclic_graph_reuses_subtrees() {
        let result = evaluate_reference(
            1,
            HUMAN_WARRIOR,
            |_| QuestState::Absent,
            |id| {
                Some(if id < 20 {
                    row(ConditionKind::All, id + 1, id + 1)
                } else {
                    row(ConditionKind::Always, 0, 0)
                })
            },
        );
        assert_eq!(result, Err(Unavailable::Limit));
    }

    #[test]
    fn invalid_race_class_masks_stay_unavailable_under_reverse_and_not() {
        for (race, class) in [(0, 0), (0x100, 0), (0, 0x20)] {
            for flags in [0, 1] {
                let mut invalid = row(ConditionKind::RaceClass, race, class);
                invalid.flags = flags;
                let rows = HashMap::from([(1, invalid), (2, row(ConditionKind::Not, 1, 0))]);
                for id in [1, 2] {
                    assert_eq!(
                        evaluate_reference(
                            id,
                            HUMAN_WARRIOR,
                            |_| QuestState::Absent,
                            |id| rows.get(&id).copied(),
                        ),
                        Err(Unavailable::Operands(14))
                    );
                }
            }
        }
    }

    #[test]
    fn a_missing_quest_subject_stays_unavailable_under_reverse_not_and_or() {
        for kind in [
            ConditionKind::QuestRewarded,
            ConditionKind::QuestTaken,
            ConditionKind::QuestAbsent,
        ] {
            for flags in [0, 1] {
                let mut missing_quest = row(kind, 777, 0);
                missing_quest.flags = flags;
                let rows = HashMap::from([
                    (1, missing_quest),
                    (2, row(ConditionKind::Not, 1, 0)),
                    (3, row(ConditionKind::Always, 0, 0)),
                    (4, row(ConditionKind::Any, 3, 1)),
                ]);
                for id in [1, 2, 4] {
                    assert_eq!(
                        evaluate_reference(
                            id,
                            HUMAN_WARRIOR,
                            |_| QuestState::Missing,
                            |id| rows.get(&id).copied(),
                        ),
                        Err(Unavailable::MissingQuest(777))
                    );
                }
            }
        }
    }
}
