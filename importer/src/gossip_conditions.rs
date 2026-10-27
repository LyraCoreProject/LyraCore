use std::collections::BTreeMap;

use lyracore_shared::constants::gossip_condition;
use lyracore_shared::gossip::{
    evaluate_option, evaluate_reference, CharacterFacts, Condition, QuestState,
};

use crate::dump::{field, gmo, parse_table};

pub(super) type Conditions = BTreeMap<u32, Condition>;

pub(super) fn read(dump: &str) -> Conditions {
    let mut conditions = BTreeMap::new();
    for row in parse_table(dump, "conditions") {
        let Ok(id) = field(&row, 0).parse::<u32>() else {
            eprintln!("gossip condition: invalid source id {}", field(&row, 0));
            continue;
        };
        if id == 0 {
            continue;
        }
        let parsed = (|| {
            let kind = field(&row, 1).parse().ok()?;
            let value1 = field(&row, 2).parse().ok()?;
            let value2 = field(&row, 3).parse().ok()?;
            let (value3, value4, flags) = if row.len() >= 7 {
                (
                    field(&row, 4).parse().ok()?,
                    field(&row, 5).parse().ok()?,
                    field(&row, 6).parse().ok()?,
                )
            } else if row.len() == 4 || row.len() == 5 {
                (0, 0, 0)
            } else {
                return None;
            };
            Some(Condition {
                kind,
                values: [value1, value2, value3, value4],
                flags,
            })
        })();
        let unavailable = Condition {
            kind: i32::MIN,
            values: [0; 4],
            flags: 0,
        };
        let condition = parsed.unwrap_or_else(|| {
            eprintln!("gossip condition {id}: malformed operands; unavailable");
            unavailable
        });
        if conditions.insert(id, condition).is_some() {
            eprintln!("gossip condition {id}: duplicate source id; unavailable");
            conditions.insert(id, unavailable);
        }
    }
    conditions
}

pub(super) fn sql_rows(conditions: &Conditions) -> Vec<String> {
    conditions
        .iter()
        .map(|(id, condition)| {
            let Condition {
                kind,
                values: [a, b, c, d],
                flags,
            } = condition;
            format!("({id},{kind},{a},{b},{c},{d},{flags})")
        })
        .collect()
}

pub(super) fn resolve(row: &[String], conditions: &Conditions) -> (u32, u32, u32) {
    let facts = CharacterFacts {
        race: 1,
        class: 1,
        level: 1,
    };
    let diagnostic = |reason: &str| {
        eprintln!(
            "gossip menu {} option {}: {reason}; unavailable",
            field(row, gmo::MENU_ID),
            field(row, gmo::ID),
        )
    };
    // Check the 15-column layout first; its column 11 is box_money, not an inline condition.
    let reference_column = if row.len() >= gmo::NCOLS_15_CONDITION_FK {
        Some(gmo::CONDITION_ID_15)
    } else if row.len() >= gmo::NCOLS_WITH_INLINE_COND {
        None
    } else if row.len() >= gmo::NCOLS_WITH_CONDITION_FK {
        Some(gmo::CONDITION_ID)
    } else {
        return (gossip_condition::NONE, 0, 0);
    };
    if let Some(column) = reference_column {
        let Ok(id) = field(row, column).parse::<u32>() else {
            diagnostic("invalid condition reference");
            return (gossip_condition::NEVER, 0, 0);
        };
        if id == 0 {
            return (gossip_condition::NONE, 0, 0);
        }
        if let Err(reason) = evaluate_reference(
            id,
            facts,
            |_| QuestState::Absent,
            |id| conditions.get(&id).copied(),
        ) {
            diagnostic(&format!("condition {id}: {reason:?}"));
        }
        return (gossip_condition::REFERENCE, id, 0);
    }
    let parsed = (|| {
        Some((
            field(row, gmo::INLINE_COND_TYPE).parse::<i32>().ok()?,
            field(row, gmo::INLINE_COND_VALUE1).parse::<u32>().ok()?,
            field(row, gmo::INLINE_COND_VALUE2).parse::<u32>().ok()?,
        ))
    })();
    let Some((kind, a, b)) = parsed else {
        diagnostic("malformed inline condition");
        return (gossip_condition::NEVER, 0, 0);
    };
    let encoded = match kind {
        0 => gossip_condition::NONE,
        6 => gossip_condition::TEAM,
        8 => gossip_condition::QUEST_REWARDED,
        9 => gossip_condition::QUEST_TAKEN,
        14 => gossip_condition::RACE_CLASS,
        15 => gossip_condition::LEVEL,
        22 => gossip_condition::QUEST_NONE,
        _ => {
            diagnostic(&format!("unsupported inline condition kind {kind}"));
            return (gossip_condition::NEVER, a, b);
        }
    };
    if let Err(reason) = evaluate_option(encoded, a, b, facts, |_| QuestState::Absent, |_| None) {
        diagnostic(&format!("inline condition: {reason:?}"));
        return (gossip_condition::NEVER, a, b);
    }
    (encoded, a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lyracore_shared::gossip::Unavailable;

    #[test]
    fn compound_operands_and_reverse_flags_survive_import() {
        let rows = read("INSERT INTO `conditions` VALUES (501,-2,502,503,504,505,1),(502,15,20,1,0,0,0),(503,6,469,0,0,0,0),(504,14,1,128,0,0,0),(505,8,777,0,0,0,0);");
        assert_eq!(
            rows[&501],
            Condition {
                kind: -2,
                values: [502, 503, 504, 505],
                flags: 1
            }
        );
        assert_eq!(rows[&504].values, [1, 128, 0, 0]);
        assert_eq!(sql_rows(&rows)[0], "(501,-2,502,503,504,505,1)");
    }

    #[test]
    fn malformed_and_duplicate_source_rows_stay_unavailable() {
        let rows = read("INSERT INTO `conditions` VALUES (1,9,777,'bad'),(2,0,0,0),(2,0,0,0),(3,-3,1,0),(4,-3,2,0),(5,-3,6,0),(6,-3,5,0);");
        for (id, unavailable) in [
            (1, Unavailable::Unsupported(i32::MIN)),
            (2, Unavailable::Unsupported(i32::MIN)),
            (3, Unavailable::Unsupported(i32::MIN)),
            (4, Unavailable::Unsupported(i32::MIN)),
            (5, Unavailable::Cycle(5)),
            (6, Unavailable::Cycle(6)),
        ] {
            assert_eq!(
                evaluate_reference(
                    id,
                    CharacterFacts::default(),
                    |_| QuestState::Absent,
                    |id| rows.get(&id).copied()
                ),
                Err(unavailable)
            );
        }
    }
}
